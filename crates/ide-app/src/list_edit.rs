//! Edit mode of the profiles / repos / widgets lists, like the session list's (`session_edit.rs`):
//! the list header's toggle puts check boxes on the rows (a click checks a row instead of
//! picking it), and a bar above the list checks all and deletes the checked items (one dialog for
//! all of them; repositories are unregistered, their files stay) or ends edit mode with 완료.

use std::collections::BTreeSet;

use iced::widget::{Column, button, container, row, space, text};
use iced::{Alignment, Element, Fill, Task, Widget as _};
use ide_client::ProtoError;

use crate::app::{App, Message, Modal};
use crate::changes_view::check_box;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::session_edit::tri_state;
use crate::style;
use crate::view::{error_box, tip};

type El<'a> = Element<'a, Message>;

/// The list being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Profiles,
    Repos,
    Widgets,
}

/// Edit mode state (not saved; left when the page changes).
#[derive(Debug, Default)]
pub struct ListEdit {
    pub kind: Option<ListKind>,
    /// Checked ids (may hold ids no longer listed; only listed ones count).
    pub checked: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub enum ListEditMsg {
    /// The list header's toggle: into / out of edit mode (checks cleared either way).
    Toggle(ListKind),
    /// 완료.
    Done,
    /// A row was clicked: (un)check it.
    Row(String),
    /// The bar's check-all box.
    CheckAll(bool),
    AskDelete,
    DeleteConfirm,
    Deleted(ListKind, Vec<(String, Result<(), ProtoError>)>),
}

/// The delete dialog: the checked items of one list.
pub struct ListDeleteForm {
    pub kind: ListKind,
    pub ids: Vec<String>,
    pub error: Option<String>,
    pub busy: bool,
}

fn lm(m: ListEditMsg) -> Message {
    Message::ListEdit(m)
}

impl App {
    /// `kind`'s list is in edit mode.
    pub(crate) fn list_editing(&self, kind: ListKind) -> bool {
        self.list_edit.kind == Some(kind)
    }

    /// The (id, name) of every item `kind`'s list shows.
    fn list_items(&self, kind: ListKind) -> Vec<(String, String)> {
        match kind {
            ListKind::Profiles => {
                self.configs.iter().map(|c| (c.slug.clone(), c.name.clone())).collect()
            }
            ListKind::Repos => self.repos.iter().map(|r| (r.id.clone(), r.name.clone())).collect(),
            ListKind::Widgets => {
                self.widgets.list.iter().map(|w| (w.id.clone(), w.name.clone())).collect()
            }
        }
    }

    /// The listed items that are checked, in list order.
    fn list_picked(&self, kind: ListKind) -> Vec<String> {
        self.list_items(kind)
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| self.list_edit.checked.contains(id))
            .collect()
    }

    /// A row's check box in edit mode (the row's own press toggles it).
    pub(crate) fn list_row_box<'a>(&self, id: &str) -> El<'a> {
        let on = self.list_edit.checked.contains(id);
        check_box(Some(on), lm(ListEditMsg::Row(id.to_owned())))
    }

    /// A row's press: (un)check it in edit mode, `normal` otherwise.
    pub(crate) fn list_row_press(&self, kind: ListKind, id: &str, normal: Message) -> Message {
        if self.list_editing(kind) {
            lm(ListEditMsg::Row(id.to_owned()))
        } else {
            normal
        }
    }

    /// A row's highlight: checked in edit mode, `selected` otherwise.
    pub(crate) fn list_row_lit(&self, kind: ListKind, id: &str, selected: bool) -> bool {
        if self.list_editing(kind) {
            self.list_edit.checked.contains(id)
        } else {
            selected
        }
    }

    /// The list header's edit mode toggle.
    pub(crate) fn list_edit_toggle(&self, kind: ListKind) -> El<'_> {
        let on = self.list_editing(kind);
        tip(
            button(icons::icon(
                Icon::ListCheck,
                16.0,
                if on { style::ACCENT_TEXT } else { style::DIM },
            ))
            .padding(3)
            .style(style::icon_toggle(on))
            .on_press(lm(ListEditMsg::Toggle(kind)))
            .boxed(),
            if on { tr::session_edit_end() } else { tr::list_edit_start() },
        )
    }

    pub(crate) fn on_list_edit(&mut self, m: ListEditMsg) -> Task<Message> {
        match m {
            ListEditMsg::Toggle(kind) => {
                let on = !self.list_editing(kind);
                self.list_edit = ListEdit {
                    kind: on.then_some(kind),
                    checked: BTreeSet::new(),
                };
            }
            ListEditMsg::Done => self.list_edit = ListEdit::default(),
            ListEditMsg::Row(id) => {
                let c = &mut self.list_edit.checked;
                if !c.remove(&id) {
                    c.insert(id);
                }
            }
            ListEditMsg::CheckAll(on) => {
                let Some(kind) = self.list_edit.kind else {
                    return Task::none();
                };
                let ids = self.list_items(kind).into_iter().map(|(id, _)| id);
                let c = &mut self.list_edit.checked;
                c.clear();
                if on {
                    c.extend(ids);
                }
            }
            ListEditMsg::AskDelete => {
                let Some(kind) = self.list_edit.kind else {
                    return Task::none();
                };
                let ids = self.list_picked(kind);
                if !self.connected() || ids.is_empty() {
                    return Task::none();
                }
                self.modal = Some(Modal::ListDelete(ListDeleteForm {
                    kind,
                    ids,
                    error: None,
                    busy: false,
                }));
            }
            ListEditMsg::DeleteConfirm => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::ListDelete(f)) = &mut self.modal else {
                    return Task::none();
                };
                if f.busy {
                    return Task::none();
                }
                f.busy = true;
                f.error = None;
                let (kind, ids) = (f.kind, f.ids.clone());
                // One at a time: the server refuses a profile / repository sessions still use,
                // and the rest go on.
                return Task::perform(
                    async move {
                        let mut out = Vec::with_capacity(ids.len());
                        for id in ids {
                            let r = match kind {
                                ListKind::Profiles => c.delete_configuration(&id).await,
                                ListKind::Repos => c.unregister_repo(&id).await,
                                ListKind::Widgets => c.widget_delete(&id).await,
                            };
                            out.push((id, r));
                        }
                        out
                    },
                    move |r| lm(ListEditMsg::Deleted(kind, r)),
                );
            }
            ListEditMsg::Deleted(kind, results) => {
                let names = self.list_items(kind);
                let name_of = |id: &str| {
                    names
                        .iter()
                        .find(|(i, _)| i == id)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| id.to_owned())
                };
                let mut failed = Vec::new();
                let mut gone = Vec::new();
                for (id, r) in results {
                    crate::script::log(&format!("list {kind:?} delete {id}: {r:?}"));
                    match r {
                        Ok(()) => gone.push(id),
                        Err(e) => {
                            let line = format!("{}: {}", name_of(&id), i18n::proto_error(&e));
                            failed.push((id, line));
                        }
                    }
                }
                for id in &gone {
                    self.list_edit.checked.remove(id);
                }
                let reload = self.after_list_delete(kind, &gone);
                if failed.is_empty() {
                    if matches!(self.modal, Some(Modal::ListDelete(_))) {
                        self.modal = None;
                    }
                } else if let Some(Modal::ListDelete(f)) = &mut self.modal {
                    f.busy = false;
                    f.ids = failed.iter().map(|(id, _)| id.clone()).collect();
                    f.error = Some(
                        failed.into_iter().map(|(_, e)| e).collect::<Vec<_>>().join("\n"),
                    );
                }
                return reload;
            }
        }
        Task::none()
    }

    /// Forget the deleted items (what a single delete on the page does) and reload the list.
    fn after_list_delete(&mut self, kind: ListKind, gone: &[String]) -> Task<Message> {
        match kind {
            ListKind::Profiles => {
                let f = &mut self.settings;
                if f.profile_sel.as_ref().is_some_and(|s| gone.contains(s)) {
                    f.profile_sel = None;
                    f.cfg_edit = None;
                    f.profile_delete_armed = false;
                }
                if self.ui.configuration.as_ref().is_some_and(|s| gone.contains(s)) {
                    self.ui.configuration = None;
                    self.save_ui();
                }
                self.refresh()
            }
            ListKind::Repos => {
                self.repos.retain(|r| !gone.contains(&r.id));
                let f = &mut self.settings;
                if f.repo_selected.as_ref().is_some_and(|s| gone.contains(s)) {
                    f.repo_selected = None;
                    f.repo_confirm = false;
                }
                self.load_repos()
            }
            ListKind::Widgets => self.load_widgets(),
        }
    }

    /// The bar above the list in edit mode (like the session list's).
    pub(crate) fn list_edit_bar(&self, kind: ListKind) -> El<'_> {
        let picked = self.list_picked(kind);
        let all_ids: Vec<String> = self.list_items(kind).into_iter().map(|(id, _)| id).collect();
        let all = tri_state(all_ids.iter().map(String::as_str), &self.list_edit.checked);
        let all_box = tip(
            check_box(all, lm(ListEditMsg::CheckAll(all != Some(true)))),
            tr::session_edit_check_all(),
        );
        let count = if picked.is_empty() {
            text(tr::list_edit_none()).size(12).color(style::DIM)
        } else {
            text(tr::session_edit_count(picked.len() as i64)).size(12)
        };
        let label = match kind {
            ListKind::Repos => tr::repo_unregister(),
            _ => tr::delete(),
        };
        let delete = button(
            row![icons::icon(Icon::Trash, 13.0, style::ON_ACCENT), text(label).size(12)]
                .spacing(4)
                .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(style::danger)
        .on_press_maybe(
            (self.connected() && !picked.is_empty()).then_some(lm(ListEditMsg::AskDelete)),
        );
        let done = button(text(tr::session_edit_done()).size(12))
            .padding([4, 8])
            .style(style::secondary)
            .on_press(lm(ListEditMsg::Done));
        row![
            all_box,
            container(count.wrapping(text::Wrapping::None).ellipsis(text::Ellipsis::End))
                .width(Fill),
            delete,
            done,
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .boxed()
    }

    /// The delete dialog of the list in edit mode.
    pub(crate) fn list_delete_card<'a>(&'a self, f: &'a ListDeleteForm) -> El<'a> {
        let n = f.ids.len() as i64;
        let (title, body, confirm) = match f.kind {
            ListKind::Profiles => (
                tr::list_delete_profiles_title(n),
                tr::list_delete_profiles_body(),
                tr::session_edit_delete_confirm(n),
            ),
            ListKind::Repos => (
                tr::list_delete_repos_title(n),
                tr::list_delete_repos_body(),
                tr::list_delete_repos_confirm(n),
            ),
            ListKind::Widgets => (
                tr::list_delete_widgets_title(n),
                tr::list_delete_widgets_body(),
                tr::session_edit_delete_confirm(n),
            ),
        };
        let items = self.list_items(f.kind);
        let names: Vec<&str> = f
            .ids
            .iter()
            .map(|id| {
                items
                    .iter()
                    .find(|(i, _)| i == id)
                    .map(|(_, n)| n.as_str())
                    .unwrap_or(id.as_str())
            })
            .collect();
        let mut c = Column::<El<'a>>::new()
            .spacing(10)
            .push(text(title).size(18).font(style::bold()).boxed())
            .push(text(body).size(13).color(style::DIM).boxed())
            .push(text(names.join(", ")).size(12).color(style::FAINT).boxed());
        if let Some(e) = &f.error {
            c = c.push(error_box(e));
        }
        c = c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                button(text(if f.busy { tr::deleting() } else { confirm }).size(13))
                    .padding([6, 14])
                    .style(style::danger)
                    .on_press_maybe(
                        (!f.busy && self.connected()).then_some(lm(ListEditMsg::DeleteConfirm))
                    ),
            ]
            .spacing(8)
            .boxed(),
        );
        c.width(480).boxed()
    }
}
