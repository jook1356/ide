//! The session list's branch badge, checked by the server (`docs/branch-badge.md`; Hermes
//! Desk's `SessionBranchBadge`, `lib/sessionIndicators.ts`, `lib/sessionBranchSync.ts`).
//!
//! Each repo of a session is checked (edits, commits to push, its PR, and where its commits
//! are: git's answer only, see [`landing`]) and the repos are aggregated into one
//! [`BranchState`]. The last result of every session is kept
//! in `<data>/branch-badges.json`, so a server that starts again (and an app that opens again)
//! shows the previous badges at once instead of 확인 중, and checks them again in the
//! background. Checks run only while a client is connected: every [`FULL_EVERY`] all
//! sessions, and sooner a session whose files or git state changed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

use ide_core::{Session, SessionWorktree};
use ide_proto::{BranchBadge, BranchState, Landed, Notify, PrState, ProtoError, Request, Response};

use crate::git_actions::{succeeds, try_git};

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

/// All repos of a session as one: sums of the counts, any edit / PR / conflict, and the
/// nearest place every worked-in repo's work landed (a repo without work of its own does not
/// count; a repo whose PR was merged on GitHub counts as `pr_merged`, not as unlanded).
pub(crate) fn aggregate(bs: &[BranchState]) -> BranchState {
    if bs.is_empty() {
        return BranchState::default();
    }
    let any = |f: fn(&BranchState) -> Option<bool>| {
        if bs.iter().any(|b| f(b) == Some(true)) {
            Some(true)
        } else if bs.iter().all(|b| f(b) == Some(false)) {
            Some(false)
        } else {
            None
        }
    };
    let pushing: Vec<&BranchState> = bs.iter().filter(|b| b.ahead > 0).collect();
    let pending: Vec<&BranchState> = bs.iter().filter(|b| !b.pr_merged).collect();
    let unlanded = pending.iter().map(|b| b.unlanded).sum();
    BranchState {
        dirty: any(|b| b.dirty),
        ahead: pushing.iter().map(|b| b.ahead).sum(),
        unpublished: !pushing.is_empty() && pushing.iter().all(|b| b.unpublished),
        open_pr: any(|b| b.open_pr),
        pr_conflict: bs.iter().any(|b| b.open_pr == Some(true) && b.pr_conflict),
        unlanded,
        landed: if unlanded == 0 {
            pending.iter().filter_map(|b| b.landed).min()
        } else {
            None
        },
        pr_merged: unlanded == 0 && bs.iter().any(|b| b.pr_merged),
    }
}

/// The newest commit made on a branch, from its reflog (`%H<TAB>%gs`, newest first): commits,
/// amends, cherry-picks, reverts, `am`, and a rebase of such work. Merges, fast-forwards,
/// resets and the branch's creation are not work of its own (a child synced from its parent
/// has none).
fn work_tip(reflog: &str) -> Option<&str> {
    const OWN: [&str; 6] = ["commit:", "commit (amend):", "commit (initial):", "cherry-pick:", "revert:", "am:"];
    let mut work = None;
    for line in reflog.lines().rev() {
        let Some((sha, subject)) = line.trim().split_once('\t') else {
            continue;
        };
        let own = OWN.iter().any(|p| subject.starts_with(p));
        if own || (work.is_some() && subject.starts_with("rebase")) {
            work = Some(sha);
        }
    }
    work
}

/// Where the work checked out in `dir` is, by git's ancestry only (`docs/branch-badge.md`):
/// `targets` are refs nearest first (the parents' branches, the local base, `origin/<base>`).
/// Returns the commits (merges left out) no target contains and, when that is none, the
/// farthest target that contains the branch's own work (`None` without work of its own).
/// Targets that do not exist, or that are the branch itself, are skipped.
fn landing(dir: &Path, targets: &[(String, Landed)]) -> (u32, Option<Landed>) {
    let branch = try_git(dir, &["symbolic-ref", "--quiet", "HEAD"]).unwrap_or_default();
    let present: Vec<&(String, Landed)> = targets
        .iter()
        .filter(|(r, _)| {
            *r != branch
                && try_git(dir, &["rev-parse", "--verify", "--quiet", &format!("{r}^{{commit}}")]).is_some()
        })
        .collect();
    if present.is_empty() {
        return (0, None);
    }
    let mut args = vec!["rev-list", "--count", "--no-merges", "HEAD", "--not"];
    args.extend(present.iter().map(|(r, _)| r.as_str()));
    let unlanded = try_git(dir, &args).and_then(|n| n.parse().ok()).unwrap_or(0);
    if unlanded > 0 || branch.is_empty() {
        return (unlanded, None);
    }
    let reflog = try_git(dir, &["reflog", "show", "--format=%H%x09%gs", &branch]).unwrap_or_default();
    let Some(work) =
        work_tip(&reflog).filter(|w| succeeds(dir, &["merge-base", "--is-ancestor", w, "HEAD"]))
    else {
        return (0, None);
    };
    let landed = present
        .iter()
        .filter(|(r, _)| succeeds(dir, &["merge-base", "--is-ancestor", work, r]))
        .map(|(_, l)| *l)
        .max();
    (0, landed)
}

impl UserCtx {
    /// Where a session's work in `wt`'s repo may land, nearest first: the branches of its
    /// parents (separate children; a shared child has its parent's branch), then the base of
    /// the root workspace session, locally and on `origin`. Original-repo sessions: none.
    fn targets(&self, list: &[Session], s: &Session, wt: &SessionWorktree) -> Vec<(String, Landed)> {
        let mut out = Vec::new();
        let mut cur = s;
        while let Some(pid) = &cur.parent {
            let Some(p) = list.iter().find(|x| &x.id == pid) else {
                break;
            };
            if !cur.shared
                && let Some(pw) = p.worktrees.iter().find(|w| w.repo == wt.repo)
                && !pw.branch.is_empty()
            {
                out.push((format!("refs/heads/{}", pw.branch), Landed::Parent));
            }
            cur = p;
        }
        if cur.is_workspace()
            && let Some(rw) = cur.worktrees.iter().find(|w| w.repo == wt.repo)
        {
            let base = self.session_base(cur, rw);
            if !base.is_empty() {
                out.push((format!("refs/heads/{base}"), Landed::LocalBase));
                out.push((format!("refs/remotes/origin/{base}"), Landed::Base));
            }
        }
        out
    }

    /// One repo of a session: edits and commits to push through the same requests the app's
    /// footer uses, where its commits are ([`landing`]), and its PR (skipped without a remote;
    /// a failing PR call leaves that part unknown). Blocking.
    fn branch_repo(&self, list: &[Session], s: &Session, wt: &SessionWorktree) -> Result<BranchState, ProtoError> {
        let unexpected = || ProtoError::new("internal").arg("detail", "unexpected response");
        let call = |req: Request| self.handle_blocking(req);
        let (session, repo) = (s.id.to_string(), wt.repo.to_string());
        let Response::GitStatus(st) = call(Request::GitStatus {
            session: session.clone(),
            repo: repo.clone(),
            base: None,
        })?
        else {
            return Err(unexpected());
        };
        let Response::GitSyncStatus(sync) = call(Request::GitSyncStatus {
            session: session.clone(),
            repo: repo.clone(),
            fetch: false,
            merged_head: None,
        })?
        else {
            return Err(unexpected());
        };
        let targets = self.targets(list, s, wt);
        let (unlanded, landed) = landing(&wt.path, &targets);
        let mut b = BranchState {
            dirty: Some(!(st.staged.is_empty() && st.unstaged.is_empty() && st.untracked.is_empty())),
            ahead: sync.ahead,
            unpublished: sync.unpublished,
            unlanded,
            landed,
            ..Default::default()
        };
        // A branch not on the remote needs publishing only while it has work to land and lands
        // through the remote: a child lands in its parent's local branch.
        let child = targets.first().is_some_and(|(_, l)| *l == Landed::Parent);
        if b.unpublished && (child || unlanded == 0) {
            b.ahead = 0;
            b.unpublished = false;
        }
        if sync.no_remote {
            b.open_pr = Some(false);
            return Ok(b);
        }
        match call(Request::PrStatus { session, repo }) {
            Ok(Response::PrStatus(Some(pr))) => {
                b.open_pr = Some(pr.state == PrState::Open);
                b.pr_conflict = pr.state == PrState::Open && pr.mergeable_state == "dirty";
                // Merged on GitHub, yet git does not see the commits in a target (a squash, or
                // not fetched yet): reported apart, while nothing was added since.
                b.pr_merged = pr.state == PrState::Merged
                    && unlanded > 0
                    && b.dirty == Some(false)
                    && sync.ahead == 0;
            }
            Ok(Response::PrStatus(None)) => b.open_pr = Some(false),
            _ => {}
        }
        Ok(b)
    }

    /// Check one session's repos. Blocking.
    fn check_branch(&self, list: &[Session], s: &Session) -> BranchBadge {
        let mut all = Vec::with_capacity(s.worktrees.len());
        for w in &s.worktrees {
            match self.branch_repo(list, s, w) {
                Ok(b) => all.push(b),
                Err(e) => return BranchBadge::Failed(e),
            }
        }
        BranchBadge::Known(aggregate(&all))
    }

    /// Check the sessions `which` picks (all with `None`; with the children of each, whose
    /// work may have landed in it); tells clients about changed badges and saves them.
    /// Blocking.
    fn check_branches(&self, which: Option<&HashSet<String>>) {
        let list: Vec<Session> = self.sessions.lock().unwrap().list().to_vec();
        let mut changed = false;
        if which.is_none() {
            let live: HashSet<String> = list.iter().map(|s| s.id.to_string()).collect();
            changed |= self.branch_badges.retain(&live);
        }
        let which = which.map(|w| with_descendants(&list, w));
        let which = which.as_ref();
        for s in &list {
            let id = s.id.to_string();
            // Worktrees still being created have nothing to check yet.
            if s.worktrees.is_empty() || s.preparing() || which.is_some_and(|w| !w.contains(&id)) {
                continue;
            }
            let badge = self.check_branch(&list, s);
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

/// `ids` and every session below them.
fn with_descendants(list: &[Session], ids: &HashSet<String>) -> HashSet<String> {
    let mut out = ids.clone();
    loop {
        let more: Vec<String> = list
            .iter()
            .filter(|s| s.parent.as_ref().is_some_and(|p| out.contains(p.as_str())) && !out.contains(s.id.as_str()))
            .map(|s| s.id.to_string())
            .collect();
        if more.is_empty() {
            return out;
        }
        out.extend(more);
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
    fn repos_aggregate() {
        // Counts add up; any edit / PR shows.
        let a = aggregate(&[b(|x| x.unlanded = 1), b(|x| { x.unlanded = 2; x.dirty = Some(true) })]);
        assert_eq!((a.unlanded, a.dirty, a.landed), (3, Some(true), None));
        let a = aggregate(&[b(|x| { x.ahead = 1; x.unpublished = true }), b(|x| x.ahead = 2)]);
        assert_eq!((a.ahead, a.unpublished), (3, false));
        // A conflict needs an open PR.
        assert!(!aggregate(&[b(|x| x.pr_conflict = true)]).pr_conflict);
        assert!(aggregate(&[b(|x| { x.open_pr = Some(true); x.pr_conflict = true })]).pr_conflict);
        // Landed: the nearest place of the worked-in repos; an idle repo does not count.
        let a = aggregate(&[b(|x| x.landed = Some(Landed::Base)), b(|_| {})]);
        assert_eq!(a.landed, Some(Landed::Base));
        let a = aggregate(&[b(|x| x.landed = Some(Landed::Base)), b(|x| x.landed = Some(Landed::Parent))]);
        assert_eq!(a.landed, Some(Landed::Parent));
        // Work left anywhere: not landed.
        let a = aggregate(&[b(|x| x.landed = Some(Landed::Base)), b(|x| x.unlanded = 1)]);
        assert_eq!((a.unlanded, a.landed), (1, None));
        // A PR merged on GitHub (squash) is reported apart and does not count as unlanded.
        let a = aggregate(&[b(|x| { x.unlanded = 2; x.pr_merged = true }), b(|x| x.landed = Some(Landed::Base))]);
        assert_eq!((a.unlanded, a.pr_merged, a.landed), (0, true, Some(Landed::Base)));
        let a = aggregate(&[b(|x| { x.unlanded = 2; x.pr_merged = true }), b(|x| x.unlanded = 1)]);
        assert_eq!((a.unlanded, a.pr_merged), (1, false));
    }

    #[test]
    fn work_is_what_was_committed_on_the_branch() {
        // Newest first, as `git reflog show` prints it.
        let log = "f\tmerge ide/p: Fast-forward\ne\tcommit: two\nd\tcommit: one\nc\tbranch: Created from ide/p\n";
        assert_eq!(work_tip(log), Some("e"));
        // Only synced from the parent / created: no work of its own.
        assert_eq!(work_tip("b\tmerge ide/p: Fast-forward\na\tbranch: Created from HEAD\n"), None);
        assert_eq!(work_tip("b\trebase (finish): refs/heads/x onto 1\na\tbranch: Created from HEAD\n"), None);
        // A rebase of work, an amend, a cherry-pick are work.
        assert_eq!(work_tip("c\trebase (finish): refs/heads/x onto 1\nb\tcommit: w\na\tbranch: Created\n"), Some("c"));
        assert_eq!(work_tip("b\tcommit (amend): w\na\tbranch: Created\n"), Some("b"));
        assert_eq!(work_tip("b\tcherry-pick: w\na\tbranch: Created\n"), Some("b"));
        // A merge commit (e.g. a sync with conflicts) is not.
        assert_eq!(work_tip("b\tcommit (merge): Merge ide/p\na\tbranch: Created\n"), None);
    }

    /// Run git in `dir` with a fixed identity and none of the user's config.
    fn g(dir: &Path, args: &[&str]) {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", dir.join("no-global-config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(o.status.success() || args[0] == "merge", "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    }

    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        g(dir, &["add", file]);
        g(dir, &["commit", "-q", "-m", text]);
    }

    #[test]
    fn landing_follows_git_ancestry() {
        let base = std::env::var_os("IDE_TEST_TMP").map_or_else(std::env::temp_dir, PathBuf::from);
        let dir = base.join(format!("ide-landing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        g(&dir, &["init", "-q", "-b", "main"]);
        commit(&dir, "a.txt", "1");
        let targets = |parent: &str| {
            vec![
                (format!("refs/heads/{parent}"), Landed::Parent),
                ("refs/heads/main".to_owned(), Landed::LocalBase),
                ("refs/remotes/origin/main".to_owned(), Landed::Base),
            ]
        };
        let at = |branch: &str, parent: &str| {
            g(&dir, &["checkout", "-q", branch]);
            landing(&dir, &targets(parent))
        };
        g(&dir, &["branch", "ide/p"]);
        g(&dir, &["checkout", "-q", "-b", "ide/c", "ide/p"]);
        assert_eq!(landing(&dir, &targets("ide/p")), (0, None), "no work yet");
        commit(&dir, "a.txt", "child");
        assert_eq!(landing(&dir, &targets("ide/p")), (1, None));

        // The parent merges it with a conflict it resolves its own way: still merged for git.
        g(&dir, &["checkout", "-q", "ide/p"]);
        commit(&dir, "a.txt", "parent");
        g(&dir, &["merge", "-q", "--no-ff", "ide/c"]);
        std::fs::write(dir.join("a.txt"), "resolved").unwrap();
        g(&dir, &["add", "a.txt"]);
        g(&dir, &["commit", "-q", "--no-edit"]);
        assert_eq!(at("ide/c", "ide/p"), (0, Some(Landed::Parent)));

        // The parent lands in the base: so does the child's work.
        g(&dir, &["checkout", "-q", "main"]);
        g(&dir, &["merge", "-q", "--no-ff", "--no-edit", "ide/p"]);
        assert_eq!(at("ide/c", "ide/p"), (0, Some(Landed::LocalBase)));
        // New work: not landed again.
        commit(&dir, "c.txt", "more");
        assert_eq!(landing(&dir, &targets("ide/p")), (1, None));

        // A squash is not a merge for git.
        g(&dir, &["checkout", "-q", "-b", "ide/s", "main"]);
        commit(&dir, "s.txt", "s1");
        commit(&dir, "s.txt", "s2");
        g(&dir, &["checkout", "-q", "main"]);
        g(&dir, &["merge", "-q", "--squash", "ide/s"]);
        g(&dir, &["commit", "-q", "-m", "squashed"]);
        assert_eq!(at("ide/s", "main"), (2, None));

        // A fast-forward is.
        g(&dir, &["checkout", "-q", "-b", "ide/f", "main"]);
        commit(&dir, "f.txt", "f");
        g(&dir, &["checkout", "-q", "main"]);
        g(&dir, &["merge", "-q", "--ff-only", "ide/f"]);
        assert_eq!(at("ide/f", "main"), (0, Some(Landed::LocalBase)));

        // Only synced from its parent (a fast-forward): no work of its own, nothing landed.
        g(&dir, &["checkout", "-q", "-b", "ide/y", "ide/p"]);
        g(&dir, &["merge", "-q", "--ff-only", "main"]);
        assert_eq!(landing(&dir, &targets("ide/p")), (0, None));

        g(&dir, &["checkout", "-q", "main"]);
        let _ = std::fs::remove_dir_all(&dir);
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
