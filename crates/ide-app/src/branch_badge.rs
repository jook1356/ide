//! The session list's branch badge (Hermes Desk's `SessionBranchBadge`,
//! `lib/sessionIndicators.ts`): the server checks each session's repos (edits, commits to
//! push, its PR, where its commits are by git's ancestry), aggregates them and keeps the
//! result across restarts (`ide-server` `service/branch_badges.rs`, `docs/branch-badge.md`);
//! this shows the most pressing state: PR 열림 / PR 충돌, 커밋 필요, 푸시 필요 / 게시 필요,
//! 커밋 보유, PR 머지됨, 부모에 머지됨, 머지됨, else 변경 없음 (확인 중 before the first
//! check, 조회 실패 when it failed).

use iced::widget::{container, text};
use iced::{Color, Element, Widget as _};
use ide_client::{BranchBadge, BranchState, Landed};

use crate::app::{App, Message};
use crate::i18n::tr;
use crate::style;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dirty,
    Ahead,
    Publish,
    Pr,
    PrConflict,
    Unlanded,
    PrMerged,
    InParent,
    Merged,
}

/// The most pressing state (Desk's `branchKind`; `None`: nothing to show).
fn kind(b: &BranchState) -> Option<Kind> {
    if b.open_pr == Some(true) {
        return Some(if b.pr_conflict { Kind::PrConflict } else { Kind::Pr });
    }
    if b.dirty == Some(true) {
        return Some(Kind::Dirty);
    }
    if b.ahead > 0 {
        return Some(if b.unpublished { Kind::Publish } else { Kind::Ahead });
    }
    if b.unlanded > 0 {
        return Some(Kind::Unlanded);
    }
    if b.pr_merged {
        return Some(Kind::PrMerged);
    }
    match b.landed? {
        Landed::Parent => Some(Kind::InParent),
        Landed::LocalBase | Landed::Base => Some(Kind::Merged),
    }
}

impl App {
    /// The badge of a session row (nothing for sessions without repos).
    pub(crate) fn branch_badge(&self, badge: Option<&BranchBadge>, has_repos: bool) -> Option<El<'_>> {
        if !has_repos {
            return None;
        }
        let (label, color, tooltip) = match badge {
            None => (tr::badge_checking(), style::DIM, tr::badge_checking()),
            Some(BranchBadge::Failed(e)) => (tr::badge_failed(), style::DIM, crate::i18n::proto_error(e)),
            Some(BranchBadge::Known(b)) => match kind(b) {
                None => (tr::badge_none(), style::DIM, tr::badge_none_tip()),
                Some(k) => {
                    let (label, color) = match k {
                        Kind::Dirty => (tr::badge_dirty(), style::YELLOW),
                        Kind::Ahead => (tr::badge_ahead(), style::ACCENT),
                        Kind::Publish => (tr::badge_publish(), style::ACCENT),
                        Kind::Pr => (tr::badge_pr(), style::GREEN),
                        Kind::PrConflict => (tr::badge_pr_conflict(), style::ORANGE),
                        Kind::Unlanded => (tr::badge_unmerged(), style::CYAN),
                        Kind::PrMerged => (tr::badge_pr_merged(), style::PURPLE),
                        Kind::InParent => (tr::badge_in_parent(), style::PURPLE),
                        Kind::Merged => (tr::badge_merged(), style::PURPLE),
                    };
                    let tip_text = match k {
                        Kind::Unlanded => tr::badge_unmerged_tip(i64::from(b.unlanded)),
                        Kind::Ahead | Kind::Publish => tr::badge_ahead_tip(i64::from(b.ahead)),
                        Kind::PrMerged => tr::badge_pr_merged_tip(),
                        Kind::InParent => tr::badge_in_parent_tip(),
                        Kind::Merged if b.landed == Some(Landed::LocalBase) => tr::badge_merged_local_tip(),
                        Kind::Merged => tr::badge_merged_tip(),
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
    fn the_most_pressing_state_wins() {
        assert_eq!(kind(&b(|_| {})), None);
        assert_eq!(kind(&b(|x| x.dirty = Some(true))), Some(Kind::Dirty));
        assert_eq!(kind(&b(|x| x.ahead = 2)), Some(Kind::Ahead));
        assert_eq!(kind(&b(|x| { x.ahead = 2; x.unpublished = true })), Some(Kind::Publish));
        assert_eq!(kind(&b(|x| { x.open_pr = Some(true); x.dirty = Some(true) })), Some(Kind::Pr));
        assert_eq!(kind(&b(|x| { x.open_pr = Some(true); x.pr_conflict = true })), Some(Kind::PrConflict));
        assert_eq!(kind(&b(|x| x.unlanded = 3)), Some(Kind::Unlanded));
        // Commits to push come before commits to merge.
        assert_eq!(kind(&b(|x| { x.unlanded = 3; x.ahead = 1 })), Some(Kind::Ahead));
        assert_eq!(kind(&b(|x| x.pr_merged = true)), Some(Kind::PrMerged));
        assert_eq!(kind(&b(|x| x.landed = Some(Landed::Parent))), Some(Kind::InParent));
        assert_eq!(kind(&b(|x| x.landed = Some(Landed::LocalBase))), Some(Kind::Merged));
        assert_eq!(kind(&b(|x| x.landed = Some(Landed::Base))), Some(Kind::Merged));
        // New edits after a merge show.
        assert_eq!(kind(&b(|x| { x.landed = Some(Landed::Base); x.dirty = Some(true) })), Some(Kind::Dirty));
    }
}
