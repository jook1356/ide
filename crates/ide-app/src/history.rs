//! The side panel's History tab (Hermes Desk `ChangesPanel` History, `lib/repoPanelCache.ts`
//! `loadRepositoryHistory`, `lib/relativeTime.ts`, `AuthorAvatar`): the shown repo's commits
//! newest first (`git.log`), Desk's "not pushed yet" marks (`git.unpushed`), and the files of
//! unfolded commits (`git.commit_files`). Pure state and formatting; drawing is in
//! `detail_view.rs`, loading in `detail.rs`.

use std::collections::{HashMap, HashSet};

use iced::Color;
use ide_client::{CommitInfo, GIT_LOG_PAGE, GitChange};

use crate::i18n::tr;

/// Files of an unfolded commit.
#[derive(Debug, Clone)]
pub enum CommitFiles {
    Loading,
    Loaded(Vec<GitChange>),
    Failed(String),
}

/// History of the shown repo (reset when another repo is shown).
#[derive(Debug, Default)]
pub struct HistoryState {
    /// Newest first.
    pub commits: Vec<CommitInfo>,
    pub has_more: bool,
    /// The first page arrived (until then the list shows "loading").
    pub loaded: bool,
    /// A load was started for this repo (the next one is a refresh).
    pub requested: bool,
    /// A "more" page is in flight.
    pub loading_more: bool,
    /// Full ids of the commits not on a remote yet (capped at `GIT_UNPUSHED_MAX`).
    pub unpushed: HashSet<String>,
    /// Unfolded commits (by full id).
    pub expanded: HashSet<String>,
    /// Files of commits that were unfolded (commits never change, so they stay cached).
    pub files: HashMap<String, CommitFiles>,
    /// Incremented per refresh; answers of older ones are dropped.
    pub seq: u64,
}

impl HistoryState {
    /// How many commits a refresh asks for: at least a page, and every commit already shown
    /// (Desk: `Math.max(200, commits.length)`).
    pub fn refresh_limit(&self) -> u32 {
        GIT_LOG_PAGE.max(u32::try_from(self.commits.len()).unwrap_or(u32::MAX))
    }

    pub fn commit(&self, sha: &str) -> Option<&CommitInfo> {
        self.commits.iter().find(|c| c.sha == sha)
    }

    /// Append a "more" page (commits already listed are skipped, like Desk's
    /// `appendHistoryPage`).
    pub fn append(&mut self, page: Vec<CommitInfo>, has_more: bool) {
        for c in page {
            if !self.commits.iter().any(|x| x.sha == c.sha) {
                self.commits.push(c);
            }
        }
        self.has_more = has_more;
    }

    /// Fold / unfold a commit; `true` when its files must be loaded now.
    pub fn toggle(&mut self, sha: &str) -> bool {
        if !self.expanded.remove(sha) {
            self.expanded.insert(sha.to_owned());
            if !matches!(
                self.files.get(sha),
                Some(CommitFiles::Loaded(_) | CommitFiles::Loading)
            ) {
                self.files.insert(sha.to_owned(), CommitFiles::Loading);
                return true;
            }
        }
        false
    }

    /// The History tab's badge text (Desk: `500+` at the cap).
    pub fn badge(&self) -> Option<String> {
        let n = self.unpushed.len();
        (n > 0).then(|| {
            if n >= ide_client::GIT_UNPUSHED_MAX as usize {
                format!("{}+", ide_client::GIT_UNPUSHED_MAX)
            } else {
                n.to_string()
            }
        })
    }
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Desk's `relativeTime` buckets ("방금", "5분 전", "어제", "14일 전", "지난달", "3개월 전",
/// "작년", "2년 전").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ago {
    JustNow,
    Minutes(i64),
    Hours(i64),
    Yesterday,
    Days(i64),
    LastMonth,
    Months(i64),
    LastYear,
    Years(i64),
}

/// Desk's `relativeTime` (times in seconds).
pub fn ago(at: i64, now: i64) -> Ago {
    let s = (now - at).max(0) as f64;
    let min = (s / 60.0).round() as i64;
    let hr = (s / 3600.0).round() as i64;
    let day = (s / 86400.0).floor() as i64;
    if s < 60.0 {
        return Ago::JustNow;
    }
    if min < 60 {
        return Ago::Minutes(min);
    }
    if hr < 24 {
        return Ago::Hours(hr);
    }
    if day < 30 {
        return if day == 1 { Ago::Yesterday } else { Ago::Days(day) };
    }
    let month = (day as f64 / 30.44).floor() as i64;
    if month < 12 {
        return if month <= 1 {
            Ago::LastMonth
        } else {
            Ago::Months(month)
        };
    }
    let year = (day as f64 / 365.25).floor() as i64;
    if year <= 1 {
        Ago::LastYear
    } else {
        Ago::Years(year)
    }
}

/// [`ago`] in the UI language.
pub fn relative_time(at: i64, now: i64) -> String {
    match ago(at, now) {
        Ago::JustNow => tr::time_just_now(),
        Ago::Minutes(n) => tr::time_minutes(n),
        Ago::Hours(n) => tr::time_hours(n),
        Ago::Yesterday => tr::time_yesterday(),
        Ago::Days(n) => tr::time_days(n),
        Ago::LastMonth => tr::time_last_month(),
        Ago::Months(n) => tr::time_months(n),
        Ago::LastYear => tr::time_last_year(),
        Ago::Years(n) => tr::time_years(n),
    }
}

/// The full local date and time of a commit (`2026-11-21 14:03`; Desk: `toLocaleString`).
pub fn full_date(at: i64) -> String {
    let Ok(t) = time::OffsetDateTime::from_unix_timestamp(at) else {
        return at.to_string();
    };
    let t = time::UtcOffset::current_local_offset().map_or(t, |o| t.to_offset(o));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute()
    )
}

/// Desk's `AuthorAvatar` initials: two words -> their first letters, else the first two
/// letters (a trailing `[bot]` dropped), upper case.
pub fn initials(name: &str) -> String {
    let mut n = name.trim_end();
    if let Some(cut) = n.len().checked_sub(5)
        && n.get(cut..).is_some_and(|t| t.eq_ignore_ascii_case("[bot]"))
    {
        n = &n[..cut];
    }
    let words: Vec<&str> = n
        .trim()
        .split(|c: char| c.is_whitespace() || matches!(c, '.' | '_' | '-'))
        .filter(|w| !w.is_empty())
        .collect();
    let s: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
    };
    s.to_uppercase()
}

/// Desk's avatar colour: `hsl(hash % 360, 45%, 38%)` of the lower-cased e-mail (or name),
/// `hash = hash * 31 + UTF-16 unit` in 32 bits.
pub fn avatar_color(name: &str, email: &str) -> Color {
    let key = if email.is_empty() { name } else { email }.to_lowercase();
    let h = key
        .encode_utf16()
        .fold(0u32, |h, u| h.wrapping_mul(31).wrapping_add(u32::from(u)));
    hsl((h % 360) as f32, 0.45, 0.38)
}

fn hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color::from_rgb(r + m, g + m, b + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(sha: &str) -> CommitInfo {
        CommitInfo {
            sha: sha.into(),
            short: sha[..4].into(),
            subject: format!("subject {sha}"),
            author: "a".into(),
            email: "a@x".into(),
            date: 0,
        }
    }

    #[test]
    fn relative_time_matches_desk() {
        let now = 1_000_000_000;
        let cases = [
            (0, Ago::JustNow),
            (59, Ago::JustNow),
            (5 * 60, Ago::Minutes(5)),
            (3 * 3600, Ago::Hours(3)),
            (86_400 + 10, Ago::Yesterday),
            (14 * 86_400, Ago::Days(14)),
            (35 * 86_400, Ago::LastMonth),
            (100 * 86_400, Ago::Months(3)),
            (400 * 86_400, Ago::LastYear),
            (800 * 86_400, Ago::Years(2)),
        ];
        for (secs, want) in cases {
            assert_eq!(ago(now - secs, now), want, "{secs}s ago");
        }
        // A commit dated in the future (clock skew) is "just now".
        assert_eq!(ago(now + 50, now), Ago::JustNow);
    }

    #[test]
    fn initials_like_desk() {
        assert_eq!(initials("ide-test"), "IT");
        assert_eq!(initials("Jane Doe"), "JD");
        assert_eq!(initials("dependabot[bot]"), "DE");
        assert_eq!(initials("홍길동"), "홍길");
        assert_eq!(initials("  "), "?");
        assert_eq!(initials("j.r.r tolkien"), "JR");
    }

    #[test]
    fn avatar_color_is_stable_hsl() {
        // "a": hash 97 -> hue 97.
        let c = avatar_color("x", "A");
        let want = hsl(97.0, 0.45, 0.38);
        assert_eq!(c, want);
        assert_eq!(avatar_color("Name", ""), avatar_color("x", "name"));
        // hsl(0, 45%, 38%) = rgb(140, 53, 53).
        let red = hsl(0.0, 0.45, 0.38);
        assert!((red.r * 255.0 - 140.0).abs() < 1.0 && (red.g * 255.0 - 53.0).abs() < 1.0);
    }

    #[test]
    fn paging_toggle_and_badge() {
        let mut h = HistoryState::default();
        assert_eq!(h.refresh_limit(), GIT_LOG_PAGE);
        h.commits = vec![commit("aaaa1"), commit("bbbb2")];
        h.append(vec![commit("bbbb2"), commit("cccc3")], true);
        assert_eq!(h.commits.len(), 3);
        assert!(h.has_more);
        assert!(h.toggle("aaaa1"), "first unfold loads the files");
        assert!(h.expanded.contains("aaaa1"));
        assert!(!h.toggle("aaaa1"), "folding loads nothing");
        h.files.insert("aaaa1".into(), CommitFiles::Loaded(Vec::new()));
        assert!(!h.toggle("aaaa1"), "cached files are not loaded again");
        h.files.insert("bbbb2".into(), CommitFiles::Failed("x".into()));
        assert!(h.toggle("bbbb2"), "a failed load is retried");
        assert_eq!(h.badge(), None);
        h.unpushed.insert("aaaa1".into());
        assert_eq!(h.badge().as_deref(), Some("1"));
        h.unpushed = (0..ide_client::GIT_UNPUSHED_MAX).map(|i| i.to_string()).collect();
        assert_eq!(h.badge().as_deref(), Some("500+"));
    }

    #[test]
    fn full_date_format() {
        let s = full_date(1_700_000_000);
        assert_eq!(s.len(), 16, "{s}");
        assert!(s.starts_with("2023-11-1"), "{s}");
    }
}
