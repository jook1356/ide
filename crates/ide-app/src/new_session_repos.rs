//! The new-session dialog's repository list: most recently used repos first, and the search
//! box's filter. Pure functions so the order and matching are unit-tested apart from the view.

use std::collections::HashMap;

use ide_client::{RepoInfo, SessionInfo};

/// From this many registered repos on, the dialog shows a search box and the checked repos as
/// chips above the (scrolling) list; fewer fit on screen as they are.
pub const SEARCH_MIN_REPOS: usize = 6;

/// Most recently used first: a repo's last use is the latest creation or activity of a session
/// that has it. Repos no session uses follow in registry order (stable sort, so equal times keep
/// the registry order too). Returns indices into `repos`.
pub fn recent_order<'a>(
    repos: &[RepoInfo],
    sessions: impl IntoIterator<Item = &'a SessionInfo>,
) -> Vec<usize> {
    let mut last: HashMap<&str, u64> = HashMap::new();
    for s in sessions {
        let t = s.created_at.max(s.last_active);
        for r in &s.repos {
            let e = last.entry(r.repo.as_str()).or_default();
            *e = (*e).max(t);
        }
    }
    let mut order: Vec<usize> = (0..repos.len()).collect();
    // `None` (never used) sorts after every time.
    order.sort_by_key(|&i| std::cmp::Reverse(last.get(repos[i].id.as_str()).copied().map(|t| t + 1)));
    order
}

/// The search box's match: every whitespace-separated word of `query` occurs in the repo's name
/// or path, ignoring case. An empty query matches every repo.
pub fn matches(query: &str, repo: &RepoInfo) -> bool {
    let name = repo.name.to_lowercase();
    let path = repo.path.to_lowercase();
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .all(|w| name.contains(&w) || path.contains(&w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(id: &str, name: &str, path: &str) -> RepoInfo {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "path": path, "base": "main", "sessions": 0,
        }))
        .unwrap()
    }

    fn session(created: u64, active: u64, repos: &[&str]) -> SessionInfo {
        let mut s: SessionInfo = serde_json::from_value(serde_json::json!({
            "id": "s", "title": "", "kind": "Agent", "provider": "shell", "account": null,
            "configuration": "", "mode": "Workspace", "root": "", "repos": [],
            "created_at": created, "last_active": active, "state": "Stopped",
            "term_title": "", "term_end": 0,
        }))
        .unwrap();
        s.repos = repos
            .iter()
            .map(|r| {
                serde_json::from_value(serde_json::json!({
                    "repo": r, "name": r, "folder": r, "path": "", "branch": "", "base": "",
                }))
                .unwrap()
            })
            .collect();
        s
    }

    #[test]
    fn recent_first_then_registry_order() {
        let repos = vec![
            repo("a", "a", "/a"),
            repo("b", "b", "/b"),
            repo("c", "c", "/c"),
            repo("d", "d", "/d"),
        ];
        let sessions = [
            session(10, 0, &["b"]),
            // Activity counts as use: `d` was used last.
            session(5, 50, &["d", "b"]),
            session(20, 0, &["c"]),
        ];
        // b and d were both last used at 50 (registry order between them), then c (20), then
        // a, which no session uses.
        assert_eq!(recent_order(&repos, &sessions), vec![1, 3, 2, 0]);
        // No sessions: the registry order.
        assert_eq!(recent_order(&repos, []), vec![0, 1, 2, 3]);
    }

    #[test]
    fn unused_repos_follow_used_ones() {
        let repos = vec![repo("a", "a", "/a"), repo("b", "b", "/b")];
        assert_eq!(recent_order(&repos, &[session(0, 0, &["b"])]), vec![1, 0]);
    }

    #[test]
    fn search_matches_every_word_in_name_or_path() {
        let r = repo("a", "Frontend", r"C:\work\Acme\frontend");
        assert!(matches("", &r));
        assert!(matches("  ", &r));
        assert!(matches("front", &r));
        assert!(matches("ACME front", &r));
        assert!(!matches("acme backend", &r));
        assert!(!matches("backend", &r));
    }
}
