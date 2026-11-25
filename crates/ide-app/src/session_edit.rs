//! The session list's edit mode (Hermes Desk's `SessionEditBar` / `BulkDeleteModal`): the list
//! header's toggle puts checkboxes on the rows (a click checks a row and its child sessions
//! instead of opening it), each configuration divider checks its block, and a bar above the list
//! checks all and acts on the checked sessions: 고정 / 고정 해제 / 보관 / 보관 해제 under "…",
//! 삭제 (one dialog for all of them) and 완료.

use std::collections::BTreeSet;

use iced::widget::{Column, button, checkbox, column, container, popover, row, space, text};
use iced::{Alignment, Element, Fill, Task, Widget as _};

use crate::app::{App, Live, Message, Modal};
use crate::changes_view::check_box;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::style;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// Edit mode state (not saved: the app starts with the list in its normal mode).
#[derive(Debug, Default)]
pub struct SessionEdit {
    pub on: bool,
    /// Checked session ids (may hold ids no longer listed; only listed ones count).
    pub checked: BTreeSet<String>,
    /// The bar's "…" menu is open.
    pub menu: bool,
}

#[derive(Debug, Clone)]
pub enum EditMsg {
    /// The header's toggle: into / out of edit mode (checks cleared either way).
    Toggle,
    /// 완료.
    Done,
    /// A row was clicked: (un)check it and its child sessions.
    Row(String),
    /// Check (`true`) / uncheck these ids.
    Check(Vec<String>, bool),
    /// The bar's check-all box.
    CheckAll(bool),
    Menu(bool),
    /// Pin / unpin the checked sessions that can be pinned.
    Pin(bool),
    /// Into / out of the archive.
    Archive(bool),
    AskDelete,
    DeleteWorktrees(bool),
    DeleteConfirm,
}

/// The bulk delete dialog: the sessions, in the order they are deleted (children first).
pub struct BulkDeleteForm {
    pub ids: Vec<String>,
    pub remove_worktree: bool,
}

fn em(m: EditMsg) -> Message {
    Message::SessionEdit(m)
}

/// `ids` ordered so a session comes after every session below it (deepest first, stable
/// otherwise): the server refuses to delete a session that still has children.
pub fn delete_order(ids: &[String], depth_of: impl Fn(&str) -> usize) -> Vec<String> {
    let mut v: Vec<(usize, &String)> = ids.iter().map(|id| (depth_of(id), id)).collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.0));
    v.into_iter().map(|(_, id)| id.clone()).collect()
}

/// `id` and every session below it, given each session's parent.
pub fn with_descendants<'a>(
    id: &str,
    parents: impl Iterator<Item = (&'a str, Option<&'a str>)> + Clone,
) -> Vec<String> {
    let mut out = vec![id.to_owned()];
    let mut i = 0;
    while i < out.len() {
        let cur = out[i].clone();
        for (child, parent) in parents.clone() {
            if parent == Some(cur.as_str()) && !out.iter().any(|x| x == child) {
                out.push(child.to_owned());
            }
        }
        i += 1;
    }
    out
}

/// Check-box state of a set of ids: `Some(true)` all checked, `Some(false)` none (or empty),
/// `None` some.
pub fn tri_state<'a>(ids: impl Iterator<Item = &'a str>, checked: &BTreeSet<String>) -> Option<bool> {
    let (mut n, mut on) = (0, 0);
    for id in ids {
        n += 1;
        if checked.contains(id) {
            on += 1;
        }
    }
    if on == 0 {
        Some(false)
    } else if on == n {
        Some(true)
    } else {
        None
    }
}

impl App {
    /// Every session the list shows (in filter), archived ones included.
    fn edit_listed(&self) -> Vec<&Live> {
        self.sidebar_groups()
            .into_iter()
            .flat_map(|(_, _, v)| v)
            .collect()
    }

    /// The listed sessions that are checked.
    fn edit_picked(&self) -> Vec<&Live> {
        self.edit_listed()
            .into_iter()
            .filter(|l| self.session_edit.checked.contains(&l.info.id))
            .collect()
    }

    /// What check-all checks: the listed sessions, leaving out the archived ones of a folded
    /// 보관함 (they can't be seen; Desk's `shelved`).
    pub(crate) fn edit_checkable(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (key, _, members) in self.sidebar_groups() {
            let open = self.ui.archive_open.contains(&key);
            out.extend(
                members
                    .into_iter()
                    .filter(|l| open || !l.info.archived)
                    .map(|l| l.info.id.clone()),
            );
        }
        out
    }

    /// A group divider's check box in edit mode (its visible sessions).
    pub(crate) fn edit_group_box<'a>(&self, key: &str, members: &[&Live]) -> El<'a> {
        let open = self.ui.archive_open.contains(key);
        let ids: Vec<String> = members
            .iter()
            .filter(|l| open || !l.info.archived)
            .map(|l| l.info.id.clone())
            .collect();
        let state = tri_state(ids.iter().map(String::as_str), &self.session_edit.checked);
        check_box(state, em(EditMsg::Check(ids, state != Some(true))))
    }

    /// A row's check box in edit mode (the row's own press toggles it).
    pub(crate) fn edit_row_box<'a>(&self, id: &str) -> El<'a> {
        let on = self.session_edit.checked.contains(id);
        check_box(Some(on), em(EditMsg::Row(id.to_owned())))
    }

    fn depth_of(&self, id: &str) -> usize {
        let mut d = 0;
        let mut cur = self.live(id).and_then(|l| l.info.parent.clone());
        while let Some(p) = cur {
            d += 1;
            if d > 16 {
                break;
            }
            cur = self.live(&p).and_then(|l| l.info.parent.clone());
        }
        d
    }

    pub(crate) fn on_session_edit(&mut self, m: EditMsg) -> Task<Message> {
        let e = &mut self.session_edit;
        match m {
            EditMsg::Toggle => {
                e.on = !e.on;
                e.checked.clear();
                e.menu = false;
                self.session_menu = None;
                self.session_hover = None;
            }
            EditMsg::Done => {
                *e = SessionEdit::default();
            }
            EditMsg::Row(id) => {
                let on = !e.checked.contains(&id);
                let ids = with_descendants(
                    &id,
                    self.sessions
                        .iter()
                        .map(|l| (l.info.id.as_str(), l.info.parent.as_deref())),
                );
                set_checked(&mut self.session_edit.checked, ids, on);
            }
            EditMsg::Check(ids, on) => set_checked(&mut e.checked, ids, on),
            EditMsg::CheckAll(on) => {
                let ids = self.edit_checkable();
                let e = &mut self.session_edit;
                e.checked.clear();
                if on {
                    e.checked.extend(ids);
                }
            }
            EditMsg::Menu(open) => e.menu = open,
            EditMsg::Pin(pinned) => {
                self.session_edit.menu = false;
                let ids: Vec<String> = self
                    .edit_picked()
                    .into_iter()
                    .filter(|l| !l.info.archived)
                    .map(|l| l.info.id.clone())
                    .collect();
                return self.set_flags_all(ids, Some(pinned), None);
            }
            EditMsg::Archive(archived) => {
                self.session_edit.menu = false;
                let ids: Vec<String> = self
                    .edit_picked()
                    .into_iter()
                    .filter(|l| l.info.archived != archived)
                    .map(|l| l.info.id.clone())
                    .collect();
                // Desk: archiving (or taking out) clears the checks, the rows move.
                self.session_edit.checked.clear();
                return self.set_flags_all(ids, None, Some(archived));
            }
            EditMsg::AskDelete => {
                if !self.connected() {
                    return Task::none();
                }
                let picked: Vec<String> =
                    self.edit_picked().iter().map(|l| l.info.id.clone()).collect();
                if picked.is_empty() {
                    return Task::none();
                }
                let ids = delete_order(&picked, |id| self.depth_of(id));
                self.modal = Some(Modal::BulkDelete(BulkDeleteForm {
                    ids,
                    remove_worktree: true,
                }));
            }
            EditMsg::DeleteWorktrees(v) => {
                if let Some(Modal::BulkDelete(f)) = &mut self.modal {
                    f.remove_worktree = v;
                }
            }
            EditMsg::DeleteConfirm => {
                let Some(Modal::BulkDelete(f)) = &self.modal else {
                    return Task::none();
                };
                // Children first (`delete_order`), in the background like a single delete.
                let (ids, remove) = (f.ids.clone(), f.remove_worktree);
                self.modal = None;
                return self.start_delete(ids, remove);
            }
        }
        Task::none()
    }

    fn set_flags_all(
        &mut self,
        ids: Vec<String>,
        pinned: Option<bool>,
        archived: Option<bool>,
    ) -> Task<Message> {
        if !self.connected() {
            return Task::none();
        }
        Task::batch(ids.into_iter().map(|id| {
            Task::done(Message::SetSessionFlags(id, pinned, archived))
        }))
    }

    /// The bar above the list in edit mode (Desk's `SessionEditBar`).
    pub(crate) fn edit_bar(&self) -> El<'_> {
        let picked = self.edit_picked();
        let checkable = self.edit_checkable();
        let all = tri_state(checkable.iter().map(String::as_str), &self.session_edit.checked);
        let all_box = tip(
            check_box(all, em(EditMsg::CheckAll(all != Some(true)))),
            tr::session_edit_check_all(),
        );
        let count = if picked.is_empty() {
            text(tr::session_edit_none()).size(12).color(style::DIM)
        } else {
            text(tr::session_edit_count(picked.len() as i64)).size(12)
        };
        let online = self.connected();
        // Archived sessions aren't pinned (archiving unpins), so pin / archive act on the rest.
        let pinnable = picked.iter().any(|l| !l.info.archived);
        let restorable = picked.iter().any(|l| l.info.archived);
        let more_btn = tip(
            button(icons::icon(
                Icon::Dots,
                14.0,
                if picked.is_empty() { style::FAINT } else { style::TEXT },
            ))
            .padding([4, 6])
            .style(style::secondary)
            .on_press_maybe(
                (online && !picked.is_empty()).then_some(em(EditMsg::Menu(!self.session_edit.menu))),
            )
            .boxed(),
            tr::session_edit_more(),
        );
        let menu = self.session_edit.menu.then(|| {
            let entry = |i: Icon, label: String, msg: Option<Message>| -> El<'_> {
                let fg = if msg.is_some() { style::TEXT } else { style::FAINT };
                button(
                    row![icons::icon(i, 14.0, fg), text(label).size(13).color(fg)]
                        .spacing(8)
                        .align_y(Alignment::Center),
                )
                .padding([5, 10])
                .width(Fill)
                .style(style::row(false))
                .on_press_maybe(msg)
                .boxed()
            };
            let rule = container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER));
            container(
                column![
                    entry(Icon::Pin, tr::session_pin(), pinnable.then(|| em(EditMsg::Pin(true)))),
                    entry(
                        Icon::Pin,
                        tr::session_unpin(),
                        pinnable.then(|| em(EditMsg::Pin(false)))
                    ),
                    rule,
                    entry(
                        Icon::Archive,
                        tr::session_archive(),
                        pinnable.then(|| em(EditMsg::Archive(true)))
                    ),
                    entry(
                        Icon::ArrowBackUp,
                        tr::session_unarchive(),
                        restorable.then(|| em(EditMsg::Archive(false)))
                    ),
                ]
                .spacing(2),
            )
            .padding(4)
            .width(180)
            .style(style::modal_card)
        });
        let more = popover(more_btn, menu)
            .position(iced::widget::popover::Position::Bottom)
            .gap(4)
            .on_close(em(EditMsg::Menu(false)));
        let delete = button(
            row![
                icons::icon(Icon::Trash, 13.0, style::ON_ACCENT),
                text(tr::delete()).size(12)
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(style::danger)
        .on_press_maybe((online && !picked.is_empty()).then_some(em(EditMsg::AskDelete)));
        let done = button(text(tr::session_edit_done()).size(12))
            .padding([4, 8])
            .style(style::secondary)
            .on_press(em(EditMsg::Done));
        row![
            all_box,
            container(count.wrapping(text::Wrapping::None).ellipsis(text::Ellipsis::End))
                .width(Fill),
            more,
            delete,
            done,
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .boxed()
    }

    /// The bulk delete dialog (Desk's `BulkDeleteModal`).
    pub(crate) fn bulk_delete_card<'a>(&'a self, f: &'a BulkDeleteForm) -> El<'a> {
        let titles: Vec<&str> = f
            .ids
            .iter()
            .map(|id| self.live(id).map(|l| l.info.title.as_str()).unwrap_or(id.as_str()))
            .collect();
        let mut c = Column::<El<'a>>::new()
            .spacing(10)
            .push(
                text(tr::session_edit_delete_title(f.ids.len() as i64))
                    .size(18)
                    .font(style::bold())
                    .boxed(),
            )
            .push(text(tr::session_edit_delete_body()).size(13).color(style::DIM).boxed())
            .push(
                text(titles.join(", "))
                    .size(12)
                    .color(style::FAINT)
                    .boxed(),
            )
            .push(
                checkbox(f.remove_worktree)
                    .label(tr::delete_remove_worktree())
                    .on_toggle(|v| em(EditMsg::DeleteWorktrees(v)))
                    .size(16)
                    .text_size(13)
                    .boxed(),
            );
        if f.remove_worktree {
            c = c.push(
                text(tr::session_edit_delete_warning())
                    .size(12)
                    .color(style::AMBER)
                    .boxed(),
            );
        }
        c = c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press(Message::CloseModal),
                button(text(tr::session_edit_delete_confirm(f.ids.len() as i64)).size(13))
                    .padding([6, 14])
                    .style(style::danger)
                    .on_press_maybe(self.connected().then_some(em(EditMsg::DeleteConfirm))),
            ]
            .spacing(8)
            .boxed(),
        );
        c.width(480).boxed()
    }
}

fn set_checked(checked: &mut BTreeSet<String>, ids: Vec<String>, on: bool) {
    for id in ids {
        if on {
            checked.insert(id);
        } else {
            checked.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn children_are_deleted_before_their_parents() {
        let depth = |id: &str| match id {
            "p" => 0,
            "c" => 1,
            "g" => 2,
            _ => 0,
        };
        assert_eq!(delete_order(&s(&["p", "x", "c", "g"]), depth), s(&["g", "c", "p", "x"]));
    }

    #[test]
    fn a_row_takes_its_descendants() {
        let rel = [("p", None), ("c", Some("p")), ("g", Some("c")), ("o", None), ("d", Some("p"))];
        let mut got = with_descendants("p", rel.iter().copied());
        got.sort();
        assert_eq!(got, s(&["c", "d", "g", "p"]));
        assert_eq!(with_descendants("o", rel.iter().copied()), s(&["o"]));
    }

    #[test]
    fn tri_state_counts_checked_ids() {
        let checked: BTreeSet<String> = s(&["a"]).into_iter().collect();
        assert_eq!(tri_state(["a"].into_iter(), &checked), Some(true));
        assert_eq!(tri_state(["a", "b"].into_iter(), &checked), None);
        assert_eq!(tri_state(["b"].into_iter(), &checked), Some(false));
        assert_eq!(tri_state([].into_iter(), &checked), Some(false));
    }
}
