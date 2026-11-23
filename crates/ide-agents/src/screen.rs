//! Screen reading (`docs/agent-bridge.md`, "Screen detection"): what an agent CLI is doing,
//! read from its terminal screen and title by rules in a per-provider TOML manifest
//! (`crates/ide-agents/screens/<provider>.toml`). Works without hooks; providers with hooks use
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
        Ok(m)
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
}
