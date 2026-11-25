//! Screen reading (`docs/agent-bridge.md`, "Screen detection"): what an agent CLI is doing,
//! read from its terminal screen and title by rules in a per-provider TOML manifest
//! (`crates/meninx-agents/screens/<provider>.toml`). Works without hooks; providers with hooks use
//! it to see permission prompts at once and as a fallback.
//!
//! The approach and the rule model (regions of the screen, prioritized rules of `contains` /
//! `regex` / `line_regex` gates with `all` / `any` / `not`, idle when nothing matches) follow
//! herdr's agent detection (github.com/herdrdev/herdr, Apache-2.0); the code and the
//! manifests here are written for this project.
//!
//! A rule matches when all its gates match its region. The matching rule with the highest
//! `priority` wins (the earlier one on a tie); `state = "unknown"` keeps the previous state
//! (e.g. a transcript viewer covers the screen). No match: idle.

use serde::Deserialize;

/// What the screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenState {
    Idle,
    Working,
    /// An approval, question or other dialog waits for the user.
    Blocked,
}

/// What a rule reads.
#[derive(Debug, Clone, Copy)]
pub struct ScreenInput<'a> {
    /// The visible screen, one string per row (trailing blanks trimmed).
    pub lines: &'a [String],
    /// The terminal title (OSC 0/2).
    pub title: &'a str,
}

/// A parsed manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenManifest {
    pub id: String,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Dialogs Meninx may answer by itself (`docs/auto-answers.md`).
    #[serde(default)]
    pub answers: Vec<AutoAnswer>,
}

/// A dialog Meninx answers by typing keys, when the user left it on for the agent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoAnswer {
    /// Stable id naming the kind of dialog (`trust_workspace`, `trust_added_dir`): the user's
    /// on / off setting and the UI text (provider-neutral, in the app) are keyed by it, so
    /// providers answering the same kind of dialog share an id.
    pub id: String,
    /// On unless the user turned it off.
    #[serde(default = "yes")]
    pub default: bool,
    /// See [`region`]; default `whole`.
    #[serde(default = "whole")]
    pub region: String,
    /// The dialog names a folder on the first non-blank line after the line containing this
    /// text (case-insensitive): it is answered only for folders Meninx set up for the session.
    #[serde(default)]
    pub path_after: Option<String>,
    /// What to type, one write after the other (a short pause between them). Either this or
    /// `choose`.
    #[serde(default)]
    pub keys: Vec<String>,
    /// Pick the list entry containing this text (case-insensitive): the keys come from where
    /// the cursor (`❯`) is: Down / Up to it, and Enter only once the screen shows the cursor
    /// there (a later read). Either this or `keys`.
    #[serde(default)]
    pub choose: Option<String>,
    #[serde(flatten)]
    pub gate: Gate,
}

fn yes() -> bool {
    true
}

/// A dialog on the screen that an answer matches, and how to answer it now.
#[derive(Debug)]
pub struct Answering<'a> {
    pub answer: &'a AutoAnswer,
    /// The folder the dialog names (answers with `path_after`).
    pub path: Option<String>,
    /// What to type, one write after the other.
    pub keys: Vec<String>,
}

/// A list's cursor mark at the start of a line.
fn is_cursor(l: &str) -> bool {
    l.trim_start().starts_with('❯')
}

/// The next keys toward the entry containing `choose` (case-insensitive) of the list under the
/// cursor: Down / Up from the cursor's line to the entry's, or Enter when the cursor is on it
/// (`None`: no such entry with a cursor next to it, or other text between them). Never both in
/// one go: a move the CLI misses (keys typed right after the dialog is drawn, or while the
/// machine is busy) must not leave an Enter on the first entry, often "No, exit" (seen
/// 2026-11-25). The entries are consecutive lines.
fn choose_keys(lines: &[String], choose: &str) -> Option<Vec<String>> {
    let want = choose.to_lowercase();
    let target = lines.iter().rposition(|l| l.to_lowercase().contains(&want))?;
    let near = |i: usize| i.abs_diff(target) <= 8;
    let cursor = (0..lines.len()).filter(|&i| near(i) && is_cursor(&lines[i])).min_by_key(|&i| i.abs_diff(target))?;
    let (lo, hi) = (cursor.min(target), cursor.max(target));
    // A list: no blank line between the cursor and the entry.
    if lines[lo..=hi].iter().any(|l| l.trim().is_empty()) {
        return None;
    }
    if cursor == target {
        return Some(vec!["\r".to_owned()]);
    }
    let step = if target > cursor { "\u{1b}[B" } else { "\u{1b}[A" };
    Some(vec![step.to_owned(); hi - lo])
}

/// The first paragraph after the last line containing `needle` (case-insensitive): its lines
/// trimmed and joined up to the next blank line (the last title: a redraw may leave an earlier
/// copy of the dialog's title above). A long folder wraps onto several rows (seen 2026-11-25,
/// classic renderer: `...\workspaces\0mww7zl` / `8ck`); the first row alone named another folder.
fn path_after(lines: &[String], needle: &str) -> Option<String> {
    let needle = needle.to_lowercase();
    let at = lines.iter().rposition(|l| l.to_lowercase().contains(&needle))?;
    let path: String = lines[at + 1..]
        .iter()
        .map(|l| l.trim())
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty())
        .collect();
    (!path.is_empty()).then_some(path)
}

/// One rule.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    /// `idle` / `working` / `blocked` / `unknown` (keep the previous state).
    pub state: String,
    #[serde(default)]
    pub priority: i32,
    /// See [`region`]; default `whole`.
    #[serde(default = "whole")]
    pub region: String,
    #[serde(flatten)]
    pub gate: Gate,
}

fn whole() -> String {
    "whole".to_owned()
}

/// Matchers, all of which must hold.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    /// Every needle appears (case-insensitive).
    #[serde(default)]
    pub contains: Vec<String>,
    /// Every pattern matches the region text.
    #[serde(default, with = "regexes")]
    pub regex: Vec<regex::Regex>,
    /// Every pattern matches some line of the region.
    #[serde(default, with = "regexes")]
    pub line_regex: Vec<regex::Regex>,
    /// Every nested gate matches.
    #[serde(default)]
    pub all: Vec<Gate>,
    /// At least one nested gate matches (when there are any).
    #[serde(default)]
    pub any: Vec<Gate>,
    /// No nested gate matches.
    #[serde(default)]
    pub not: Vec<Gate>,
}

mod regexes {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<regex::Regex>, D::Error> {
        let v: Vec<String> = Vec::deserialize(d)?;
        v.iter()
            .map(|s| regex::Regex::new(s).map_err(serde::de::Error::custom))
            .collect()
    }
}

impl Gate {
    fn positive(&self) -> bool {
        !self.contains.is_empty()
            || !self.regex.is_empty()
            || !self.line_regex.is_empty()
            || !self.all.is_empty()
            || !self.any.is_empty()
    }

    fn matches(&self, text: &str, lower: &str) -> bool {
        self.contains
            .iter()
            .all(|n| lower.contains(&n.to_lowercase()))
            && self.regex.iter().all(|r| r.is_match(text))
            && self
                .line_regex
                .iter()
                .all(|r| text.lines().any(|l| r.is_match(l)))
            && self.all.iter().all(|g| g.matches(text, lower))
            && (self.any.is_empty() || self.any.iter().any(|g| g.matches(text, lower)))
            && !self.not.iter().any(|g| g.matches(text, lower))
    }
}

impl ScreenManifest {
    /// Parse a manifest; every rule needs a known state and a positive matcher.
    pub fn parse(text: &str) -> Result<Self, String> {
        let m: ScreenManifest = toml::from_str(text).map_err(|e| e.to_string())?;
        for r in &m.rules {
            if !["idle", "working", "blocked", "unknown"].contains(&r.state.as_str()) {
                return Err(format!("rule {}: unknown state {:?}", r.id, r.state));
            }
            if !r.gate.positive() {
                return Err(format!("rule {}: no positive matcher", r.id));
            }
            if region(&r.region, &ScreenInput { lines: &[], title: "" }).is_none() {
                return Err(format!("rule {}: unknown region {:?}", r.id, r.region));
            }
        }
        for a in &m.answers {
            if !a.gate.positive() {
                return Err(format!("answer {}: no positive matcher", a.id));
            }
            if region(&a.region, &ScreenInput { lines: &[], title: "" }).is_none() {
                return Err(format!("answer {}: unknown region {:?}", a.id, a.region));
            }
            match (&a.choose, a.keys.is_empty()) {
                (Some(c), true) if !c.trim().is_empty() => {}
                (None, false) if a.keys.iter().all(|k| !k.is_empty()) => {}
                (Some(_), false) => return Err(format!("answer {}: keys or choose, not both", a.id)),
                _ => return Err(format!("answer {}: no keys", a.id)),
            }
        }
        Ok(m)
    }

    /// A dialog on the screen Meninx could answer, and the folder it names (when the answer
    /// asks for one; an answer whose folder is not found does not match).
    pub fn answer(&self, input: &ScreenInput<'_>) -> Option<Answering<'_>> {
        self.answers.iter().find_map(|a| {
            let text = region(&a.region, input)?;
            if text.is_empty() || !a.gate.matches(&text, &text.to_lowercase()) {
                return None;
            }
            let path = match &a.path_after {
                Some(n) => Some(path_after(input.lines, n)?),
                None => None,
            };
            let keys = match &a.choose {
                Some(c) => choose_keys(input.lines, c)?,
                None => a.keys.clone(),
            };
            Some(Answering { answer: a, path, keys })
        })
    }

    /// What the screen shows: `None` = keep the previous state (an `unknown` rule won).
    pub fn detect(&self, input: &ScreenInput<'_>) -> Option<ScreenState> {
        self.winner(input).map_or(Some(ScreenState::Idle), |r| match r.state.as_str() {
            "working" => Some(ScreenState::Working),
            "blocked" => Some(ScreenState::Blocked),
            "idle" => Some(ScreenState::Idle),
            _ => None,
        })
    }

    /// The CLI waits at its input with nothing covering it: an explicit `idle` rule wins and
    /// no dialog Meninx answers is shown. A queued prompt is typed in only then: a start dialog
    /// (folder trust, one per added folder) would take its keys. The no-match fallback is not
    /// enough (the blank screen between two start dialogs matches nothing).
    pub fn at_input(&self, input: &ScreenInput<'_>) -> bool {
        self.answer(input).is_none() && self.winner(input).is_some_and(|r| r.state == "idle")
    }

    /// The winning rule (diagnostics, tests).
    pub fn winner(&self, input: &ScreenInput<'_>) -> Option<&Rule> {
        let mut best: Option<&Rule> = None;
        for r in &self.rules {
            let Some(text) = region(&r.region, input) else {
                continue;
            };
            if text.is_empty() || !r.gate.matches(&text, &text.to_lowercase()) {
                continue;
            }
            if best.is_none_or(|b| r.priority > b.priority) {
                best = Some(r);
            }
        }
        best
    }
}

/// A horizontal rule line (`────`).
fn is_rule(l: &str) -> bool {
    let t = l.trim();
    let run = t.chars().take_while(|c| *c == '─').count();
    run > 0 && (run >= 3 || t.chars().count() == run)
}

/// A prompt line (`>` / `›` alone or followed by a space, also inside a box: `│ > ...`; `❯`).
fn is_prompt(l: &str) -> bool {
    let t = l.trim_start().trim_start_matches('│').trim_start();
    t == ">" || t == "›" || t.starts_with("> ") || t.starts_with("› ") || t.starts_with('❯')
}

/// A response line of a transcript (`• ...`, `■`, `✗`, `✓`).
fn is_response(l: &str) -> bool {
    matches!(l.trim_start().chars().next(), Some('•' | '■' | '✗' | '✓' | '⏺'))
}

fn join(lines: &[String]) -> String {
    lines.join("\n")
}

/// The text of a region (`None`: unknown region):
/// - `title`: the terminal title.
/// - `whole`: the screen.
/// - `bottom(N)` / `bottom_non_empty(N)`: the last N rows / from the Nth non-blank row from the
///   bottom; `top_non_empty(N)`: up to the Nth non-blank row.
/// - `after_last_prompt`: after the last prompt line (else the screen).
/// - `before_current_prompt` / `without_current_prompt`: the current prompt is the last prompt
///   line with no response line after it; the text before it (else the screen) / the screen
///   when there is none (else empty).
/// - `after_last_rule`: after the last horizontal rule (else the screen).
/// - `prompt_box`: between the prompt box's top border (the second rule from the bottom) and
///   the next rule; `above_prompt_box`: before that border.
pub fn region(spec: &str, input: &ScreenInput<'_>) -> Option<String> {
    let lines = input.lines;
    let n_arg = |name: &str| -> Option<usize> {
        spec.strip_prefix(name)?
            .strip_prefix('(')?
            .strip_suffix(')')?
            .trim()
            .parse()
            .ok()
    };
    let rules: Vec<usize> = (0..lines.len()).filter(|&i| is_rule(&lines[i])).collect();
    let box_top = rules.len().checked_sub(2).map(|i| rules[i]);
    let last_prompt = (0..lines.len()).rev().find(|&i| is_prompt(&lines[i]));
    let current_prompt =
        last_prompt.filter(|&p| !lines[p + 1..].iter().any(|l| is_response(l)));
    Some(match spec {
        "title" => input.title.to_owned(),
        "whole" => join(lines),
        "after_last_prompt" => match last_prompt {
            Some(p) => join(&lines[p + 1..]),
            None => join(lines),
        },
        "before_current_prompt" => match current_prompt {
            Some(p) => join(&lines[..p]),
            None => join(lines),
        },
        "without_current_prompt" => match current_prompt {
            Some(_) => String::new(),
            None => join(lines),
        },
        "after_last_rule" => match rules.last() {
            Some(&r) => join(&lines[r + 1..]),
            None => join(lines),
        },
        "prompt_box" => match box_top {
            Some(t) => {
                let end = rules.iter().find(|&&r| r > t).copied().unwrap_or(lines.len());
                join(&lines[t + 1..end])
            }
            None => String::new(),
        },
        "above_prompt_box" => match box_top {
            Some(t) => join(&lines[..t]),
            None => String::new(),
        },
        _ => {
            if let Some(n) = n_arg("bottom_non_empty") {
                let non_empty: Vec<usize> =
                    (0..lines.len()).filter(|&i| !lines[i].trim().is_empty()).collect();
                let from = non_empty.len().checked_sub(n).map_or(0, |i| non_empty[i]);
                join(&lines[from..])
            } else if let Some(n) = n_arg("top_non_empty") {
                let non_empty: Vec<usize> =
                    (0..lines.len()).filter(|&i| !lines[i].trim().is_empty()).collect();
                let to = non_empty.get(n.saturating_sub(1)).map_or(lines.len(), |&i| i + 1);
                join(&lines[..to])
            } else {
                let n = n_arg("bottom")?;
                join(&lines[lines.len().saturating_sub(n)..])
            }
        }
    })
}

/// Parse a bundled manifest (a bad one is a build-time mistake: tests parse every one).
pub fn bundled(text: &'static str) -> ScreenManifest {
    ScreenManifest::parse(text).unwrap_or_else(|e| panic!("bundled screen manifest: {e}"))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentProvider;

    fn screen(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|r| r.to_string()).collect()
    }

    fn detect(p: &dyn AgentProvider, rows: &[&str], title: &str) -> Option<ScreenState> {
        let lines = screen(rows);
        p.screen_manifest()
            .unwrap()
            .detect(&ScreenInput { lines: &lines, title })
    }

    #[test]
    fn bundled_manifests_parse() {
        for p in [&crate::Claude as &dyn AgentProvider, &crate::Codex] {
            let m = p.screen_manifest().expect("manifest");
            assert_eq!(m.id, p.id());
            assert!(!m.rules.is_empty());
        }
        assert!(crate::Shell.screen_manifest().is_none());
    }

    #[test]
    fn manifest_errors() {
        let bad = |t: &str| ScreenManifest::parse(t).unwrap_err();
        assert!(bad("id='x'
[[rules]]
id='a'
state='busy'
contains=['x']").contains("unknown state"));
        assert!(bad("id='x'
[[rules]]
id='a'
state='idle'").contains("no positive matcher"));
        assert!(bad("id='x'
[[rules]]
id='a'
state='idle'
region='middle'
contains=['x']").contains("unknown region"));
        assert!(ScreenManifest::parse("id='x'
[[rules]]
id='a'
state='idle'
regex=['(']").is_err());
    }

    #[test]
    fn regions() {
        let lines = screen(&["a", "", "> old", "• answer", "─────", "> typed", "─────", "? for shortcuts", ""]);
        let r = |spec: &str| region(spec, &ScreenInput { lines: &lines, title: "T" }).unwrap();
        assert_eq!(r("title"), "T");
        assert_eq!(r("bottom(2)"), "? for shortcuts
");
        assert_eq!(r("bottom_non_empty(2)"), "─────
? for shortcuts
");
        assert_eq!(r("top_non_empty(2)"), "a

> old");
        assert_eq!(r("after_last_prompt"), "─────
? for shortcuts
");
        assert_eq!(r("before_current_prompt"), "a

> old
• answer
─────");
        assert_eq!(r("without_current_prompt"), "");
        assert_eq!(r("after_last_rule"), "? for shortcuts
");
        assert_eq!(r("prompt_box"), "> typed");
        assert_eq!(r("above_prompt_box"), "a

> old
• answer");
        assert!(region("nope", &ScreenInput { lines: &lines, title: "" }).is_none());
    }

    #[test]
    fn claude_screens() {
        let c = &crate::Claude;
        let idle = [
            "⏺ Done.",
            "",
            "────────────────────────────────",
            "> ",
            "────────────────────────────────",
            "  ? for shortcuts",
        ];
        assert_eq!(detect(c, &idle, "✳ Claude Code"), Some(ScreenState::Idle));
        let working = [
            "⏺ Reading files",
            "",
            "✻ Thinking… (12s · ↑ 1.2k tokens · esc to interrupt)",
            "",
            "────────────────────────────────",
            "> ",
            "────────────────────────────────",
        ];
        assert_eq!(detect(c, &working, ""), Some(ScreenState::Working));
        assert_eq!(detect(c, &idle, "⠂ Claude Code"), Some(ScreenState::Working), "title spinner");
        let permission = [
            "────────────────────────────────",
            " Bash command",
            "   npm test",
            " Do you want to proceed?",
            " ❯ 1. Yes",
            "   2. Yes, and don't ask again for npm test",
            "   3. No, and tell Claude what to do differently (esc)",
            "────────────────────────────────",
            " Enter to confirm · Esc to cancel",
        ];
        assert_eq!(detect(c, &permission, "✳ Claude Code"), Some(ScreenState::Blocked));
        let transcript = ["⏺ x", "", "  Showing detailed transcript · Ctrl+O to toggle"];
        assert_eq!(detect(c, &transcript, ""), None, "keeps the previous state");
        assert_eq!(detect(c, &["hello"], ""), Some(ScreenState::Idle), "nothing matched");
    }

    #[test]
    fn codex_screens() {
        let x = &crate::Codex;
        let working = [
            "› fix the build",
            "",
            "• Working (12s • esc to interrupt)",
            "",
            "› ",
            "  ? for shortcuts",
        ];
        assert_eq!(detect(x, &working, ""), Some(ScreenState::Working));
        let idle = ["• Fixed the build.", "", "─ Worked for 1m 05s ─", "", "› ", "  ? for shortcuts"];
        assert_eq!(detect(x, &idle, "codex"), Some(ScreenState::Idle));
        let approval = [
            "› run the tests",
            "",
            "  Would you like to run the following command?",
            "  $ cargo test",
            "› 1. Yes, proceed (y)",
            "  2. No, and tell Codex what to do differently (esc)",
            "  Press enter to confirm or esc to cancel",
        ];
        assert_eq!(detect(x, &approval, "codex"), Some(ScreenState::Blocked));
        assert_eq!(detect(x, &idle, "⠹ codex"), Some(ScreenState::Working), "title spinner");
        assert_eq!(detect(x, &idle, "Action Required"), Some(ScreenState::Blocked));
        let trust = ["> You are in C:/w/s1", "", "  Do you trust the contents of this directory?", "› 1. Yes"];
        assert_eq!(detect(x, &trust, ""), Some(ScreenState::Blocked));
    }

    #[test]
    fn claude_auto_answers() {
        let m = crate::Claude.screen_manifest().unwrap();
        let screen = |rows: &[&str]| rows.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let ws = screen(&[
            "────────────────────────────────",
            " Accessing workspace:",
            "",
            " C:/Users/jook1/AppData/Roaming/meninx/meninx/data/workspaces/0mwudwwxbug",
            "",
            " Quick safety check: Is this a project you created or one you trust?",
            " Claude Code'll be able to read, edit, and execute files here.",
            "",
            " ❯ No, exit",
            "   Yes, I trust this folder",
            "",
            " Enter to confirm · Esc to cancel",
        ]);
        let ans = |rows: &[String]| m.answer(&ScreenInput { lines: rows, title: "" });
        let got = ans(&ws).expect("workspace trust");
        assert_eq!(got.answer.id, "trust_workspace");
        assert_eq!(got.path.as_deref(), Some("C:/Users/jook1/AppData/Roaming/meninx/meninx/data/workspaces/0mwudwwxbug"));
        assert_eq!(got.keys, ["\x1b[B"], "Down to Yes only: Enter once the screen shows it there");
        assert!(got.answer.default);
        // Seen 2026-11-25 (classic renderer): the folder wraps onto a second row.
        let wrapped = screen(&[
            " Accessing workspace:",
            "",
            " C:\\Users\\사용자\\AppData\\Roaming\\meninx\\meninx\\data\\workspaces\\0mww7zl",
            " 8ck",
            "",
            " Quick safety check: Is this a project you created or one you trust?",
            "",
            " ❯ No, exit",
            "   Yes, I trust this folder",
            "",
            " Enter to confirm · Esc to cancel",
        ]);
        let got = ans(&wrapped).expect("wrapped folder");
        assert_eq!(got.path.as_deref(), Some("C:\\Users\\사용자\\AppData\\Roaming\\meninx\\meninx\\data\\workspaces\\0mww7zl8ck"));
        let dir = screen(&[
            " Adding a new directory:",
            "",
            " C:/Users/jook1/Desktop/meninx/.git",
            "",
            " This directory is part of the repository at C:/Users/jook1/Desktop/meninx.",
            " ❯ No, exit",
            "   Yes, add it",
            " Enter to confirm · Esc to cancel",
        ]);
        let got = ans(&dir).expect("added dir");
        assert_eq!((got.answer.id.as_str(), got.path.as_deref()), ("trust_added_dir", Some("C:/Users/jook1/Desktop/meninx/.git")));
        // The other footer (seen 2026-11-24, the second repository's git dir).
        let other_footer = screen(&[
            " Adding a new directory:",
            "",
            " C:/Users/jook1/Desktop/powered_ide/.git",
            "",
            " This directory is part of the repository at C:/Users/jook1/Desktop/powered_ide.",
            " Security guide",
            "",
            " ❯ No, exit",
            "   Yes, add it",
            "",
            " Esc to exit · To add it, press ↓ to Yes, then Enter",
        ]);
        let got = ans(&other_footer).expect("other footer");
        assert_eq!((got.answer.id.as_str(), got.path.as_deref()), ("trust_added_dir", Some("C:/Users/jook1/Desktop/powered_ide/.git")));
        // Seen 2026-11-24: a redraw left an earlier title above, and the cursor is on Yes (the
        // Down arrived, the Enter did not): the folder is read after the last title, and only
        // Enter is sent.
        let stuck = screen(&[
            "────────────────────",
            " Adding a new directory:",
            "",
            "──────────────────────────────────────────",
            " Adding a new directory:",
            "",
            " C:/Users/jook1/Desktop/meninx/.git",
            "",
            " This directory is part of the repository at C:/Users/jook1/Desktop/meninx.",
            " Security guide",
            "",
            "   No, exit",
            " ❯ Yes, add it",
            "",
            " Esc to exit · To add it, press ↓ to Yes, then Enter",
        ]);
        let got = ans(&stuck).expect("stuck on Yes");
        assert_eq!(got.path.as_deref(), Some("C:/Users/jook1/Desktop/meninx/.git"));
        assert_eq!(got.keys, ["\r"], "only Enter: never Down past Yes");
        // Other dialogs and a missing folder line are not answered.
        let perm = screen(&[" Do you want to run this command?", " ❯ 1. Yes", " Enter to confirm · Esc to cancel"]);
        assert!(ans(&perm).is_none());
        let cut = screen(&[" Accessing workspace:", "", " ❯ No, exit", "   Yes, I trust this folder", " Enter to confirm"]);
        let cut_path = ans(&cut).and_then(|g| g.path).expect("matched");
        assert!(cut_path.starts_with("❯ No, exit"), "no folder line: the server's folder check refuses {cut_path:?}");
        // No cursor next to the entry: nothing typed.
        let no_cursor = screen(&[" Adding a new directory:", "", " C:/x/.git", "   No, exit", "   Yes, add it", " then Enter"]);
        assert!(ans(&no_cursor).is_none());
        let bad = |extra: &str| ScreenManifest::parse(&format!("id = \"x\"\n[[answers]]\nid = \"a\"\ncontains = [\"y\"]\n{extra}")).unwrap_err();
        assert!(bad("keys = []").contains("no keys"));
        assert!(bad("keys = [\"\\r\"]\nchoose = \"yes\"").contains("not both"));
    }

    /// Seen 2026-11-24: a child session's first prompt was typed while Claude Code still showed
    /// its start dialogs (bracketed paste was already on) and the dialog took it.
    #[test]
    fn at_input_only_with_an_idle_prompt() {
        let at = |p: &dyn AgentProvider, rows: &[&str], title: &str| {
            let lines = screen(rows);
            p.screen_manifest().unwrap().at_input(&ScreenInput { lines: &lines, title })
        };
        let c = &crate::Claude;
        let idle = ["", "────────────────────────────────", "> ", "────────────────────────────────", "  ? for shortcuts"];
        assert!(at(c, &idle, ""));
        assert!(at(c, &idle, "✳ Claude Code"));
        assert!(!at(c, &idle, "⠂ Claude Code"), "working");
        let trust = [
            "────────────────────────────────",
            " Accessing workspace:",
            "",
            " C:/w/s1",
            "",
            " ❯ No, exit",
            "   Yes, I trust this folder",
            "",
            " Enter to confirm · Esc to cancel",
        ];
        assert!(!at(c, &trust, "✳ Claude Code"), "a start dialog");
        let added = [" Adding a new directory:", "", " C:/r/.git", " ❯ No, exit", "   Yes, add it", " Esc to exit · To add it, press ↓ to Yes, then Enter"];
        assert!(!at(c, &added, ""), "an answered dialog without a cancel footer");
        assert!(!at(c, &[""], ""), "blank screen between two dialogs: nothing matched");
        assert!(!at(c, &["  Claude Code v2", "  Welcome back!"], ""), "banner, no input box yet");
        let x = &crate::Codex;
        assert!(at(x, &["• Fixed.", "", "› ", "  ? for shortcuts"], "codex"));
        assert!(!at(x, &["> You are in C:/w/s1", "", "  Do you trust the contents of this directory?", "› 1. Yes"], "codex"));
    }

    #[test]
    fn choosing_list_entries() {
        let rows = |r: &[&str]| r.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let three = rows(&[" title", " ❯ 1. A", "   2. B", "   3. C", ""]);
        assert_eq!(choose_keys(&three, "3. c").unwrap(), ["\x1b[B", "\x1b[B"]);
        assert_eq!(choose_keys(&three, "1. a").unwrap(), ["\r"]);
        let up = rows(&["   1. A", "   2. B", " ❯ 3. C"]);
        assert_eq!(choose_keys(&up, "1. a").unwrap(), ["\x1b[A", "\x1b[A"]);
        // Seen 2026-11-25: the Down was lost and the Enter confirmed "No, exit". The screen
        // after a lost move still shows the cursor on No: again only a move, never Enter.
        let trust = rows(&[" ❯ No, exit", "   Yes, I trust this folder"]);
        assert_eq!(choose_keys(&trust, "yes, i trust").unwrap(), ["\x1b[B"]);
        assert!(choose_keys(&three, "nope").is_none());
        let apart = rows(&[" ❯ No", "", "   Yes"]);
        assert!(choose_keys(&apart, "yes").is_none(), "not one list");
    }
}
