//! The session list's branch badge (Hermes Desk's `SessionBranchBadge`,
//! `lib/sessionIndicators.ts`, `lib/sessionBranchSync.ts`): each repo of a session is checked
//! (edits, commits to push, its PR, work its base lacks) and the most pressing state of all
//! its repos is shown: 커밋 필요, 푸시 필요 / 게시 필요, PR 열림 / PR 충돌, 커밋 보유, 머지됨, else
//! 변경 없음 (확인 중 before the first answer, 조회 실패 when it failed). Sessions are checked
//! again every [`EVERY`].

use std::collections::HashMap;
use std::time::Duration;

use iced::widget::{container, text};
use iced::{Color, Element, Task, Widget as _};
use ide_client::{Client, PrState};

use crate::app::{App, Message};
use crate::i18n::tr;
use crate::style;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// How often the sessions' branches are checked again (Desk: 30 s).
pub const EVERY: Duration = Duration::from_secs(30);

/// One repo's state (Desk's `BranchIndicator`; `None` = not known).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BranchInd {
    pub dirty: Option<bool>,
    pub ahead: u32,
    pub unpublished: bool,
    pub open_pr: Option<bool>,
    pub pr_conflict: bool,
    pub merged: bool,
    pub unmerged: u32,
    pub unmerged_conflict: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dirty,
    Ahead,
    Publish,
    Pr,
    PrConflict,
    Unmerged,
    Merged,
}

/// A session's badge: checking, failed, or the repos' aggregated state.
#[derive(Debug, Clone, Default)]
pub enum Badge {
    #[default]
    Checking,
    Failed(String),
    Known(BranchInd),
}

/// The most pressing state (Desk's `branchKind`; `None`: nothing to show).
fn kind(b: &BranchInd) -> Option<Kind> {
    if b.open_pr == Some(true) {
        return Some(if b.pr_conflict { Kind::PrConflict } else { Kind::Pr });
    }
    if b.dirty == Some(true) {
        return Some(Kind::Dirty);
    }
    if b.merged {
        return Some(Kind::Merged);
    }
    if b.ahead > 0 {
        return Some(if b.unpublished { Kind::Publish } else { Kind::Ahead });
    }
    (b.unmerged > 0).then_some(Kind::Unmerged)
}

/// All repos of a session as one (Desk's `aggregateBranches`): outstanding work wins, merged
/// only when every repo that was worked in is merged.
pub fn aggregate(bs: &[BranchInd]) -> BranchInd {
    if bs.is_empty() {
        return BranchInd::default();
    }
    if bs.iter().any(|b| b.open_pr == Some(true) && b.pr_conflict) {
        return BranchInd {
            open_pr: Some(true),
            pr_conflict: true,
            dirty: Some(bs.iter().any(|b| b.dirty == Some(true))),
            ahead: bs.iter().filter(|b| !b.merged).map(|b| b.ahead).sum(),
            ..Default::default()
        };
    }
    if bs.iter().any(|b| b.dirty == Some(true)) {
        return BranchInd {
            dirty: Some(true),
            ..Default::default()
        };
    }
    let ahead: Vec<&BranchInd> = bs.iter().filter(|b| !b.merged && b.ahead > 0).collect();
    if !ahead.is_empty() {
        return BranchInd {
            ahead: ahead.iter().map(|b| b.ahead).sum(),
            unpublished: ahead.iter().all(|b| b.unpublished),
            ..Default::default()
        };
    }
    if bs.iter().any(|b| b.open_pr == Some(true)) {
        return BranchInd {
            open_pr: Some(true),
            ..Default::default()
        };
    }
    let unmerged: Vec<&BranchInd> = bs.iter().filter(|b| !b.merged && b.unmerged > 0).collect();
    if !unmerged.is_empty() {
        return BranchInd {
            unmerged: unmerged.iter().map(|b| b.unmerged).sum(),
            unmerged_conflict: unmerged.iter().any(|b| b.unmerged_conflict),
            ..Default::default()
        };
    }
    // A repo the session never worked in must not hide the others' merged state.
    let idle = |b: &&BranchInd| {
        !b.merged && b.dirty == Some(false) && b.open_pr == Some(false) && b.ahead == 0 && b.unmerged == 0
    };
    let worked: Vec<&BranchInd> = bs.iter().filter(|b| !idle(b)).collect();
    BranchInd {
        merged: !worked.is_empty() && worked.iter().all(|b| b.merged),
        ..Default::default()
    }
}

/// One repo of a session, asked from the server (PR calls are skipped without a remote; a
/// failing PR call leaves that part unknown).
async fn check_repo(c: &Client, sid: &str, repo: &str) -> Result<BranchInd, String> {
    let e = |e: ide_client::ProtoError| crate::i18n::proto_error(&e);
    let st = c.git_status(sid, repo, None).await.map_err(e)?;
    let sync = c.git_sync_status(sid, repo, false, None).await.map_err(e)?;
    let mut b = BranchInd {
        dirty: Some(!(st.staged.is_empty() && st.unstaged.is_empty() && st.untracked.is_empty())),
        ahead: sync.ahead,
        unpublished: sync.unpublished,
        ..Default::default()
    };
    if sync.no_remote {
        b.open_pr = Some(false);
        return Ok(b);
    }
    match c.pr_status(sid, repo).await {
        Ok(Some(pr)) => {
            b.open_pr = Some(pr.state == PrState::Open);
            b.pr_conflict = pr.state == PrState::Open && pr.mergeable_state == "dirty";
            b.merged = pr.state == PrState::Merged && b.dirty == Some(false) && b.ahead == 0;
        }
        Ok(None) => b.open_pr = Some(false),
        Err(_) => {}
    }
    if b.open_pr != Some(true)
        && !b.merged
        && let Ok(Some(u)) = c.pr_unmerged(sid, repo).await
    {
        b.unmerged = u.commits;
        b.unmerged_conflict = u.conflict;
    }
    Ok(b)
}

impl App {
    /// Check the listed sessions that have repos (`only_new`: those not checked yet).
    pub(crate) fn refresh_badges(&mut self, only_new: bool) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        for l in &self.sessions {
            let repos: Vec<String> = l.info.repos.iter().map(|r| r.repo.clone()).collect();
            if repos.is_empty() || (only_new && self.badges.contains_key(&l.info.id)) {
                continue;
            }
            let (c, sid) = (c.clone(), l.info.id.clone());
            tasks.push(Task::perform(
                async move {
                    let mut all = Vec::new();
                    for r in &repos {
                        all.push(check_repo(&c, &sid, r).await?);
                    }
                    Ok(aggregate(&all))
                },
                {
                    let sid = l.info.id.clone();
                    move |r| Message::Badge(sid.clone(), r)
                },
            ));
        }
        Task::batch(tasks)
    }

    pub(crate) fn on_badge(&mut self, sid: String, r: Result<BranchInd, String>) -> Task<Message> {
        self.badges.insert(
            sid,
            match r {
                Ok(b) => Badge::Known(b),
                Err(e) => Badge::Failed(e),
            },
        );
        Task::none()
    }

    /// The badge of a session row (nothing for sessions without repos).
    pub(crate) fn branch_badge(&self, sid: &str, has_repos: bool) -> Option<El<'_>> {
        if !has_repos {
            return None;
        }
        let badge = self.badges.get(sid).cloned().unwrap_or_default();
        let (label, color, tooltip) = match &badge {
            Badge::Checking => (tr::badge_checking(), style::DIM, tr::badge_checking()),
            Badge::Failed(e) => (tr::badge_failed(), style::DIM, e.clone()),
            Badge::Known(b) => match kind(b) {
                None => (tr::badge_none(), style::DIM, tr::badge_none_tip()),
                Some(k) => {
                    let (label, color) = match k {
                        Kind::Dirty => (tr::badge_dirty(), style::YELLOW),
                        Kind::Ahead => (tr::badge_ahead(), style::ACCENT),
                        Kind::Publish => (tr::badge_publish(), style::ACCENT),
                        Kind::Pr => (tr::badge_pr(), style::GREEN),
                        Kind::PrConflict => (tr::badge_pr_conflict(), style::ORANGE),
                        Kind::Unmerged => (tr::badge_unmerged(), style::CYAN),
                        Kind::Merged => (tr::badge_merged(), style::PURPLE),
                    };
                    let tip_text = match k {
                        Kind::Unmerged if b.unmerged_conflict => {
                            tr::badge_unmerged_conflict_tip(i64::from(b.unmerged))
                        }
                        Kind::Unmerged => tr::badge_unmerged_tip(i64::from(b.unmerged)),
                        Kind::Ahead | Kind::Publish => tr::badge_ahead_tip(i64::from(b.ahead)),
                        _ => label.clone(),
                    };
                    (label, color, tip_text)
                }
            },
        };
        let pill = container(
            text(label)
                .size(10)
                .color(color)
                .wrapping(text::Wrapping::None),
        )
            .padding([1, 6])
            .style(move |_| container::Style {
                background: Some(Color { a: 0.14, ..color }.into()),
                border: iced::Border {
                    radius: 8.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });
        Some(tip(pill.boxed(), tooltip))
    }
}

/// Badges by session id.
pub type Badges = HashMap<String, Badge>;

#[cfg(test)]
mod tests {
    use super::*;

    fn b(f: impl FnOnce(&mut BranchInd)) -> BranchInd {
        let mut x = BranchInd {
            dirty: Some(false),
            open_pr: Some(false),
            ..Default::default()
        };
        f(&mut x);
        x
    }

    #[test]
    fn the_most_pressing_state_wins() {
        assert_eq!(kind(&b(|_| {})), None);
        assert_eq!(kind(&b(|x| x.dirty = Some(true))), Some(Kind::Dirty));
        assert_eq!(kind(&b(|x| x.ahead = 2)), Some(Kind::Ahead));
        assert_eq!(kind(&b(|x| { x.ahead = 2; x.unpublished = true })), Some(Kind::Publish));
        assert_eq!(kind(&b(|x| { x.open_pr = Some(true); x.dirty = Some(true) })), Some(Kind::Pr));
        assert_eq!(kind(&b(|x| { x.open_pr = Some(true); x.pr_conflict = true })), Some(Kind::PrConflict));
        assert_eq!(kind(&b(|x| x.unmerged = 3)), Some(Kind::Unmerged));
        assert_eq!(kind(&b(|x| x.merged = true)), Some(Kind::Merged));
    }

    #[test]
    fn repos_aggregate_like_desk() {
        // Edits in one repo beat a PR in another.
        let a = aggregate(&[b(|x| x.dirty = Some(true)), b(|x| x.open_pr = Some(true))]);
        assert_eq!(kind(&a), Some(Kind::Dirty));
        // A conflict is never hidden.
        let a = aggregate(&[b(|x| x.dirty = Some(true)), b(|x| { x.open_pr = Some(true); x.pr_conflict = true })]);
        assert_eq!(kind(&a), Some(Kind::PrConflict));
        // Merged only when every worked-in repo is merged; an idle repo does not count.
        assert_eq!(kind(&aggregate(&[b(|x| x.merged = true), b(|_| {})])), Some(Kind::Merged));
        assert_eq!(kind(&aggregate(&[b(|x| x.merged = true), b(|x| x.unmerged = 1)])), Some(Kind::Unmerged));
        // Commits to push add up.
        assert_eq!(aggregate(&[b(|x| x.ahead = 1), b(|x| x.ahead = 2)]).ahead, 3);
    }
}
