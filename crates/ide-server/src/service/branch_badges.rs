//! The session list's branch badge, checked by the server (`docs/branch-badge.md`; Hermes
//! Desk's `SessionBranchBadge`, `lib/sessionIndicators.ts`, `lib/sessionBranchSync.ts`).
//!
//! Each repo of a session is checked (edits, commits to push, its PR, work its base lacks) and
//! the repos are aggregated into one [`BranchState`]. The last result of every session is kept
//! in `<data>/branch-badges.json`, so a server that starts again (and an app that opens again)
//! shows the previous badges at once instead of 확인 중, and checks them again in the
//! background. Checks run only while a client is connected: every [`FULL_EVERY`] all
//! sessions, and sooner a session whose files or git state changed.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

use ide_core::Session;
use ide_proto::{BranchBadge, BranchState, Notify, PrState, ProtoError, Request, Response};

use super::UserCtx;

/// How often every session is checked again while a client is connected (Desk: 30 s).
pub(crate) const FULL_EVERY: Duration = Duration::from_secs(30);
/// How often the loop looks for work (sessions that changed, a client that connected).
const TICK: Duration = Duration::from_secs(2);
/// A changed session is checked at most this often.
const CHANGED_MIN_GAP: Duration = Duration::from_secs(5);

/// The badges by session id, their file, and the sessions that changed since their check.
pub(super) struct BranchBadges {
    file: PathBuf,
    map: Mutex<HashMap<String, BranchBadge>>,
    dirty: Mutex<HashSet<String>>,
}

impl BranchBadges {
    /// Load the badges kept by the last server (a missing or broken file = none).
    pub(super) fn load(file: PathBuf) -> Self {
        let map = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice::<HashMap<String, BranchBadge>>(&b).ok())
            .unwrap_or_default();
        BranchBadges {
            file,
            map: Mutex::new(map),
            dirty: Mutex::new(HashSet::new()),
        }
    }

    pub(super) fn get(&self, session: &str) -> Option<BranchBadge> {
        self.map.lock().unwrap().get(session).cloned()
    }

    /// A session's files or git state changed: check it again soon.
    pub(super) fn mark(&self, session: &str) {
        self.dirty.lock().unwrap().insert(session.to_owned());
    }

    fn take_dirty(&self) -> HashSet<String> {
        std::mem::take(&mut *self.dirty.lock().unwrap())
    }

    /// Store `badge`; `true` when it changed.
    fn set(&self, session: &str, badge: BranchBadge) -> bool {
        let mut map = self.map.lock().unwrap();
        if map.get(session) == Some(&badge) {
            return false;
        }
        map.insert(session.to_owned(), badge);
        true
    }

    /// Drop the badges of sessions that are gone; `true` when any was dropped.
    fn retain(&self, live: &HashSet<String>) -> bool {
        let mut map = self.map.lock().unwrap();
        let before = map.len();
        map.retain(|k, _| live.contains(k));
        map.len() != before
    }

    fn persist(&self) {
        let json = {
            let map = self.map.lock().unwrap();
            serde_json::to_vec_pretty(&*map).expect("json")
        };
        let tmp = self.file.with_extension("json.tmp");
        let r = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, &self.file));
        if let Err(e) = r {
            tracing::warn!("branch badges not saved to {}: {e}", self.file.display());
        }
    }
}

/// All repos of a session as one (Desk's `aggregateBranches`): outstanding work wins, merged
/// only when every repo that was worked in is merged.
pub(crate) fn aggregate(bs: &[BranchState]) -> BranchState {
    if bs.is_empty() {
        return BranchState::default();
    }
    if bs.iter().any(|b| b.open_pr == Some(true) && b.pr_conflict) {
        return BranchState {
            open_pr: Some(true),
            pr_conflict: true,
            dirty: Some(bs.iter().any(|b| b.dirty == Some(true))),
            ahead: bs.iter().filter(|b| !b.merged).map(|b| b.ahead).sum(),
            ..Default::default()
        };
    }
    if bs.iter().any(|b| b.dirty == Some(true)) {
        return BranchState {
            dirty: Some(true),
            ..Default::default()
        };
    }
    let ahead: Vec<&BranchState> = bs.iter().filter(|b| !b.merged && b.ahead > 0).collect();
    if !ahead.is_empty() {
        return BranchState {
            ahead: ahead.iter().map(|b| b.ahead).sum(),
            unpublished: ahead.iter().all(|b| b.unpublished),
            ..Default::default()
        };
    }
    if bs.iter().any(|b| b.open_pr == Some(true)) {
        return BranchState {
            open_pr: Some(true),
            ..Default::default()
        };
    }
    let unmerged: Vec<&BranchState> = bs.iter().filter(|b| !b.merged && b.unmerged > 0).collect();
    if !unmerged.is_empty() {
        return BranchState {
            unmerged: unmerged.iter().map(|b| b.unmerged).sum(),
            unmerged_conflict: unmerged.iter().any(|b| b.unmerged_conflict),
            ..Default::default()
        };
    }
    // A repo the session never worked in must not hide the others' merged state.
    let idle = |b: &&BranchState| {
        !b.merged && b.dirty == Some(false) && b.open_pr == Some(false) && b.ahead == 0 && b.unmerged == 0
    };
    let worked: Vec<&BranchState> = bs.iter().filter(|b| !idle(b)).collect();
    BranchState {
        merged: !worked.is_empty() && worked.iter().all(|b| b.merged),
        ..Default::default()
    }
}

impl UserCtx {
    /// One repo of a session, through the same requests the app's footer uses (PR calls are
    /// skipped without a remote; a failing PR call leaves that part unknown). Blocking.
    fn branch_repo(&self, session: &str, repo: &str) -> Result<BranchState, ProtoError> {
        let unexpected = || ProtoError::new("internal").arg("detail", "unexpected response");
        let call = |req: Request| self.handle_blocking(req);
        let Response::GitStatus(st) = call(Request::GitStatus {
            session: session.into(),
            repo: repo.into(),
            base: None,
        })?
        else {
            return Err(unexpected());
        };
        let Response::GitSyncStatus(sync) = call(Request::GitSyncStatus {
            session: session.into(),
            repo: repo.into(),
            fetch: false,
            merged_head: None,
        })?
        else {
            return Err(unexpected());
        };
        let mut b = BranchState {
            dirty: Some(!(st.staged.is_empty() && st.unstaged.is_empty() && st.untracked.is_empty())),
            ahead: sync.ahead,
            unpublished: sync.unpublished,
            ..Default::default()
        };
        if sync.no_remote {
            b.open_pr = Some(false);
            return Ok(b);
        }
        match call(Request::PrStatus {
            session: session.into(),
            repo: repo.into(),
        }) {
            Ok(Response::PrStatus(Some(pr))) => {
                b.open_pr = Some(pr.state == PrState::Open);
                b.pr_conflict = pr.state == PrState::Open && pr.mergeable_state == "dirty";
                b.merged = pr.state == PrState::Merged && b.dirty == Some(false) && b.ahead == 0;
            }
            Ok(Response::PrStatus(None)) => b.open_pr = Some(false),
            _ => {}
        }
        if b.open_pr != Some(true)
            && !b.merged
            && let Ok(Response::PrUnmerged(Some(u))) = call(Request::PrUnmerged {
                session: session.into(),
                repo: repo.into(),
            })
        {
            b.unmerged = u.commits;
            b.unmerged_conflict = u.conflict;
        }
        Ok(b)
    }

    /// Check one session's repos. Blocking.
    fn check_branch(&self, s: &Session) -> BranchBadge {
        let sid = s.id.to_string();
        let mut all = Vec::with_capacity(s.worktrees.len());
        for w in &s.worktrees {
            match self.branch_repo(&sid, &w.repo.to_string()) {
                Ok(b) => all.push(b),
                Err(e) => return BranchBadge::Failed(e),
            }
        }
        BranchBadge::Known(aggregate(&all))
    }

    /// Check the sessions `which` picks (all with `None`); tells clients about changed badges
    /// and saves them. Blocking.
    fn check_branches(&self, which: Option<&HashSet<String>>) {
        let list: Vec<Session> = self.sessions.lock().unwrap().list().to_vec();
        let mut changed = false;
        if which.is_none() {
            let live: HashSet<String> = list.iter().map(|s| s.id.to_string()).collect();
            changed |= self.branch_badges.retain(&live);
        }
        for s in &list {
            let id = s.id.to_string();
            // Worktrees still being created have nothing to check yet.
            if s.worktrees.is_empty() || s.preparing() || which.is_some_and(|w| !w.contains(&id)) {
                continue;
            }
            let badge = self.check_branch(s);
            if self.branch_badges.set(&id, badge.clone()) {
                changed = true;
                self.notify(Notify::Branch { session: id, badge });
            }
        }
        if changed {
            self.branch_badges.persist();
        }
    }
}

/// Keep the badges current while a client is connected (see the module doc).
pub(super) fn spawn_branch_badges(ctx: Weak<UserCtx>) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    rt.spawn(async move {
        let mut last_full: Option<Instant> = None;
        let mut last_check: HashMap<String, Instant> = HashMap::new();
        loop {
            tokio::time::sleep(TICK).await;
            let Some(c) = ctx.upgrade() else {
                return;
            };
            if c.subscriber_count() == 0 {
                // The next client gets a full check when it connects.
                last_full = None;
                continue;
            }
            let now = Instant::now();
            let which = if last_full.is_none_or(|t| now.duration_since(t) >= FULL_EVERY) {
                c.branch_badges.take_dirty();
                last_full = Some(now);
                last_check.clear();
                None
            } else {
                // Changed sessions (not checked within the gap; the rest stay marked) and
                // sessions that have no badge yet (new sessions).
                let mut due = c.branch_badges.take_dirty();
                let early: Vec<String> = due
                    .iter()
                    .filter(|id| last_check.get(*id).is_some_and(|t| now.duration_since(*t) < CHANGED_MIN_GAP))
                    .cloned()
                    .collect();
                for id in early {
                    due.remove(&id);
                    c.branch_badges.mark(&id);
                }
                for s in c.sessions.lock().unwrap().list() {
                    if !s.worktrees.is_empty() && c.branch_badges.get(s.id.as_str()).is_none() {
                        due.insert(s.id.to_string());
                    }
                }
                if due.is_empty() {
                    continue;
                }
                for id in &due {
                    last_check.insert(id.clone(), now);
                }
                Some(due)
            };
            let _ = tokio::task::spawn_blocking(move || c.check_branches(which.as_ref())).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(f: impl FnOnce(&mut BranchState)) -> BranchState {
        let mut x = BranchState {
            dirty: Some(false),
            open_pr: Some(false),
            ..Default::default()
        };
        f(&mut x);
        x
    }

    #[test]
    fn repos_aggregate_like_desk() {
        // Edits in one repo beat a PR in another.
        let a = aggregate(&[b(|x| x.dirty = Some(true)), b(|x| x.open_pr = Some(true))]);
        assert_eq!(a.dirty, Some(true));
        assert_ne!(a.open_pr, Some(true));
        // A conflict is never hidden.
        let a = aggregate(&[b(|x| x.dirty = Some(true)), b(|x| { x.open_pr = Some(true); x.pr_conflict = true })]);
        assert!(a.pr_conflict && a.open_pr == Some(true));
        // Merged only when every worked-in repo is merged; an idle repo does not count.
        assert!(aggregate(&[b(|x| x.merged = true), b(|_| {})]).merged);
        let a = aggregate(&[b(|x| x.merged = true), b(|x| x.unmerged = 1)]);
        assert!(!a.merged && a.unmerged == 1);
        // Commits to push add up.
        assert_eq!(aggregate(&[b(|x| x.ahead = 1), b(|x| x.ahead = 2)]).ahead, 3);
    }

    #[test]
    fn badges_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("ide-badges-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("branch-badges.json");
        let a = BranchBadges::load(file.clone());
        assert!(a.get("s1").is_none());
        assert!(a.set("s1", BranchBadge::Known(b(|x| x.ahead = 2))));
        assert!(!a.set("s1", BranchBadge::Known(b(|x| x.ahead = 2))), "unchanged");
        a.persist();
        let b2 = BranchBadges::load(file);
        assert_eq!(b2.get("s1"), Some(BranchBadge::Known(b(|x| x.ahead = 2))));
        assert!(b2.retain(&HashSet::new()));
        assert!(b2.get("s1").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
