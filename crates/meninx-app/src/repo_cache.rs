//! Per-repo cache of the side panel (Changes / History / Explore). Picking another repo in the
//! repo dropdown keeps what the panel showed for the old repo; picking it again shows that at
//! once and refreshes only what may be outdated (stale-while-revalidate).
//!
//! What is kept: [`crate::detail::RepoView`] (tree, Changes list, History, the changes actions'
//! data, the 스크립트 list). What marks an entry outdated ([`Refresh`]):
//! - `FsChanged` (files: tree + status + stashes) / `GitChanged` (also History) of a hidden repo
//!   (every repo of the shown session is watched, `App::on_fs_notify`);
//! - an answer of the repo arriving while it is hidden (it is dropped, so the entry misses it);
//! - the detail area going stale (`Detail::stale`: unwatched while hidden, reconnect): every entry;
//! - the repo not being watched when it is shown again, or the entry being older than
//!   [`MAX_AGE`] (a safety net for changes no notification reported): everything.
//!
//! Memory: at most [`CACHE_MAX`] repos per detail area (least recently stored dropped first);
//! repos detached from the session are dropped (`App::sync_detail`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Most repos kept per detail area (sessions have a few repos; this bounds odd cases).
pub const CACHE_MAX: usize = 8;
/// Older entries are refreshed completely when shown again.
pub const MAX_AGE: Duration = Duration::from_secs(5 * 60);

/// What to load again for the shown repo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Refresh {
    /// Tree folders, `git.status` (Changes) and, after it, the stash list.
    pub files: bool,
    /// History (`git.log` + unpushed marks).
    pub history: bool,
}

impl Refresh {
    pub const ALL: Refresh = Refresh {
        files: true,
        history: true,
    };
    pub const FILES: Refresh = Refresh {
        files: true,
        history: false,
    };
    pub const HISTORY: Refresh = Refresh {
        files: false,
        history: true,
    };

    pub fn any(self) -> bool {
        self.files || self.history
    }

    pub fn merge(&mut self, other: Refresh) {
        self.files |= other.files;
        self.history |= other.history;
    }
}

struct Entry<T> {
    view: T,
    dirty: Refresh,
    stored: Instant,
}

/// Views of repos that are not shown, by repo id.
pub struct RepoCache<T> {
    entries: HashMap<String, Entry<T>>,
}

impl<T> Default for RepoCache<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<T> RepoCache<T> {
    /// Keep `view` of `repo` (`dirty`: what is already known to be outdated).
    pub fn put(&mut self, repo: String, view: T, dirty: Refresh) {
        self.put_at(repo, view, dirty, Instant::now());
    }

    fn put_at(&mut self, repo: String, view: T, dirty: Refresh, stored: Instant) {
        self.entries.insert(
            repo,
            Entry {
                view,
                dirty,
                stored,
            },
        );
        while self.entries.len() > CACHE_MAX {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.stored)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// Take `repo`'s view out with what to refresh.
    pub fn take(&mut self, repo: &str) -> Option<(T, Refresh)> {
        self.take_at(repo, Instant::now())
    }

    fn take_at(&mut self, repo: &str, now: Instant) -> Option<(T, Refresh)> {
        let e = self.entries.remove(repo)?;
        let dirty = if now.saturating_duration_since(e.stored) > MAX_AGE {
            Refresh::ALL
        } else {
            e.dirty
        };
        Some((e.view, dirty))
    }

    /// A kept view of `repo` (to store a late answer in it).
    pub fn get_mut(&mut self, repo: &str) -> Option<&mut T> {
        self.entries.get_mut(repo).map(|e| &mut e.view)
    }

    /// `repo` changed (or missed an answer): refresh `what` when it is shown again.
    pub fn invalidate(&mut self, repo: &str, what: Refresh) {
        if let Some(e) = self.entries.get_mut(repo) {
            e.dirty.merge(what);
        }
    }

    /// Nothing kept can be trusted (not watched for a while, reconnect).
    pub fn invalidate_all(&mut self) {
        for e in self.entries.values_mut() {
            e.dirty = Refresh::ALL;
        }
    }

    /// Drop the repos not in `repos` (detached from the session).
    pub fn retain(&mut self, repos: &[String]) {
        self.entries.retain(|r, _| repos.contains(r));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_view_comes_back_clean() {
        let mut c = RepoCache::default();
        c.put("aaaaaaaa".into(), 1, Refresh::default());
        assert_eq!(c.take("aaaaaaaa"), Some((1, Refresh::default())));
        // Taken out: shown again, no longer cached.
        assert_eq!(c.take("aaaaaaaa"), None);
    }

    #[test]
    fn invalidation_merges() {
        let mut c = RepoCache::default();
        c.put("a".into(), (), Refresh::default());
        c.put("b".into(), (), Refresh::default());
        c.invalidate("a", Refresh::FILES);
        c.invalidate("a", Refresh::HISTORY);
        // Unknown repos are ignored.
        c.invalidate("zzz", Refresh::ALL);
        assert_eq!(c.take("a").map(|x| x.1), Some(Refresh::ALL));
        assert_eq!(c.take("b").map(|x| x.1), Some(Refresh::default()));
        c.put("b".into(), (), Refresh::default());
        c.invalidate_all();
        assert_eq!(c.take("b").map(|x| x.1), Some(Refresh::ALL));
    }

    #[test]
    fn old_entries_refresh_everything() {
        let mut c = RepoCache::default();
        let t0 = Instant::now();
        c.put_at("a".into(), (), Refresh::default(), t0);
        assert_eq!(
            c.take_at("a", t0 + MAX_AGE + Duration::from_secs(1))
                .map(|x| x.1),
            Some(Refresh::ALL)
        );
        c.put_at("a".into(), (), Refresh::default(), t0);
        assert_eq!(
            c.take_at("a", t0 + MAX_AGE).map(|x| x.1),
            Some(Refresh::default())
        );
    }

    #[test]
    fn bounded_and_detached_repos_dropped() {
        let mut c = RepoCache::default();
        let t0 = Instant::now();
        for i in 0..CACHE_MAX + 2 {
            c.put_at(
                format!("r{i}"),
                i,
                Refresh::default(),
                t0 + Duration::from_secs(i as u64),
            );
        }
        assert_eq!(c.len(), CACHE_MAX);
        // The least recently stored went first.
        assert!(c.get_mut("r0").is_none() && c.get_mut("r1").is_none());
        assert_eq!(c.get_mut("r2").copied(), Some(2));
        c.retain(&["r3".to_owned(), "r4".to_owned()]);
        assert_eq!(c.len(), 2);
        assert!(c.get_mut("r3").is_some() && c.get_mut("r5").is_none());
    }
}
