//! Session families in the app (`docs/session-family.md`): the 하위 세션 만들기 dialog, merging
//! a child into its parent / syncing a child from it (row menu, result toast), and the session
//! list's tree order with folding.

use std::collections::HashMap;

use iced::widget::{Column, button, checkbox, column, row, space, text, text_input};
use iced::{Alignment, Element, Fill, Task, Widget as _};
use ide_client::{MergeResult, ProtoError, RepoMerge};

use crate::app::{App, Live, Message, Modal};
use crate::i18n::{self, tr};
use crate::style;

type El<'a> = Element<'a, Message>;

/// Height of the 분리 / 공유 choices in the dialog.
const CHOICE_H: f32 = 72.0;

/// Levels below a root session (the server's `ide_core::family::MAX_DEPTH`).
pub const MAX_DEPTH: usize = 2;

/// The 하위 세션 만들기 dialog.
pub struct ChildForm {
    pub parent: String,
    pub title: String,
    pub shared: bool,
    /// The parent's repos picked for a separate child.
    pub repos: Vec<String>,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum FamilyMsg {
    /// Open the dialog for a child of this session.
    Open(String),
    Title(String),
    Shared(bool),
    Repo(String, bool),
    Submit,
    #[allow(clippy::type_complexity)]
    Created(Box<Result<(ide_client::SessionInfo, Vec<ide_client::RepoFailureInfo>), ProtoError>>),
    /// Merge this child into its parent / sync it from its parent.
    Merge(String),
    Sync(String),
    /// A merge / sync finished: shown in this session's panel.
    Merged(String, Result<Vec<RepoMerge>, ProtoError>),
    /// Fold / unfold a parent's children in the list.
    ToggleFold(String),
}

fn fm(m: FamilyMsg) -> Message {
    Message::Family(m)
}

impl App {
    /// Levels above `id` (0 = a root).
    pub(crate) fn family_depth(&self, id: &str) -> usize {
        let mut n = 0;
        let mut cur = self.live(id).and_then(|l| l.info.parent.clone());
        while let Some(p) = cur {
            n += 1;
            if n > MAX_DEPTH + 1 {
                break;
            }
            cur = self.live(&p).and_then(|l| l.info.parent.clone());
        }
        n
    }

    /// Children of `id`.
    pub(crate) fn family_children(&self, id: &str) -> usize {
        self.sessions
            .iter()
            .filter(|l| l.info.parent.as_deref() == Some(id))
            .count()
    }

    /// Can `id` get a child (a workspace session above the depth limit)?
    pub(crate) fn can_have_child(&self, id: &str) -> bool {
        self.live(id).is_some_and(|l| {
            l.info.mode == ide_client::SessionMode::Workspace
                && l.info.kind == ide_client::SessionKind::Agent
        }) && self.family_depth(id) < MAX_DEPTH
    }

    /// `members` (one list group, in its order) as a tree: each session followed by its
    /// children (in the group's order), with its depth; folded parents hide their descendants.
    /// A child whose parent is not in the group shows as a root.
    pub(crate) fn family_order<'a>(&self, members: Vec<&'a Live>) -> Vec<(&'a Live, usize)> {
        let ids: Vec<&str> = members.iter().map(|l| l.info.id.as_str()).collect();
        let mut kids: HashMap<&str, Vec<&'a Live>> = HashMap::new();
        let mut roots = Vec::new();
        for l in &members {
            match l.info.parent.as_deref().filter(|p| ids.contains(p)) {
                Some(p) => kids.entry(p).or_default().push(l),
                None => roots.push(*l),
            }
        }
        let mut out = Vec::new();
        fn walk<'a>(
            l: &'a Live,
            depth: usize,
            kids: &HashMap<&str, Vec<&'a Live>>,
            folded: &std::collections::BTreeSet<String>,
            out: &mut Vec<(&'a Live, usize)>,
        ) {
            out.push((l, depth));
            if depth > MAX_DEPTH || folded.contains(&l.info.id) {
                return;
            }
            for c in kids.get(l.info.id.as_str()).into_iter().flatten() {
                walk(c, depth + 1, kids, folded, out);
            }
        }
        for r in roots {
            walk(r, 0, &kids, &self.ui.folded_families, &mut out);
        }
        out
    }

    pub(crate) fn on_family(&mut self, m: FamilyMsg) -> Task<Message> {
        match m {
            FamilyMsg::Open(parent) => {
                if !self.connected() || !self.can_have_child(&parent) {
                    return Task::none();
                }
                let repos = self
                    .live(&parent)
                    .map(|l| l.info.repos.iter().map(|r| r.repo.clone()).collect())
                    .unwrap_or_default();
                self.modal = Some(Modal::NewChild(ChildForm {
                    parent,
                    title: String::new(),
                    shared: false,
                    repos,
                    error: None,
                    busy: false,
                }));
                Task::none()
            }
            FamilyMsg::Title(t) => {
                if let Some(Modal::NewChild(f)) = &mut self.modal {
                    f.title = t;
                }
                Task::none()
            }
            FamilyMsg::Shared(v) => {
                if let Some(Modal::NewChild(f)) = &mut self.modal {
                    f.shared = v;
                }
                Task::none()
            }
            FamilyMsg::Repo(id, on) => {
                if let Some(Modal::NewChild(f)) = &mut self.modal {
                    f.repos.retain(|r| *r != id);
                    if on {
                        f.repos.push(id);
                    }
                }
                Task::none()
            }
            FamilyMsg::Submit => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::NewChild(f)) = &mut self.modal else {
                    return Task::none();
                };
                if f.busy || (!f.shared && f.repos.is_empty()) {
                    return Task::none();
                }
                f.busy = true;
                f.error = None;
                let (parent, shared, repos, title) =
                    (f.parent.clone(), f.shared, f.repos.clone(), f.title.trim().to_owned());
                crate::script::log(&format!("new child of {parent}: shared={shared} repos={repos:?}"));
                Task::perform(
                    async move {
                        c.create_child_session(&parent, shared, repos, &title, true, 0, 0)
                            .await
                    },
                    |r| fm(FamilyMsg::Created(Box::new(r))),
                )
            }
            FamilyMsg::Created(r) => match *r {
                Ok((info, failures)) => self.on_created(Ok((info, failures, Vec::new()))),
                Err(e) => {
                    if let Some(Modal::NewChild(f)) = &mut self.modal {
                        f.busy = false;
                        f.error = Some(i18n::proto_error(&e));
                    }
                    Task::none()
                }
            },
            FamilyMsg::Merge(child) | FamilyMsg::Sync(child) if self.online().is_none() => {
                let _ = child;
                Task::none()
            }
            FamilyMsg::Merge(child) => {
                let c = self.online().expect("online");
                // The result shows where the merge landed: the parent.
                let shown = self
                    .live(&child)
                    .and_then(|l| l.info.parent.clone())
                    .unwrap_or_else(|| child.clone());
                Task::perform(async move { c.merge_child(&child, false).await }, move |r| {
                    fm(FamilyMsg::Merged(shown.clone(), r))
                })
            }
            FamilyMsg::Sync(child) => {
                let c = self.online().expect("online");
                let shown = child.clone();
                Task::perform(async move { c.sync_parent(&child).await }, move |r| {
                    fm(FamilyMsg::Merged(shown.clone(), r))
                })
            }
            FamilyMsg::Merged(session, r) => {
                use crate::notices::{Notice, Text, Tone};
                let tone = match &r {
                    Err(_) => Tone::Error,
                    Ok(m) if m.iter().any(|x| matches!(x.result, MergeResult::Failed { .. })) => {
                        Tone::Error
                    }
                    Ok(m) if m.iter().any(|x| matches!(x.result, MergeResult::Conflicts { .. })) => {
                        Tone::Warning
                    }
                    Ok(_) => Tone::Success,
                };
                let msg = match r {
                    Ok(m) => merge_summary(&m),
                    Err(e) => i18n::proto_error(&e),
                };
                crate::script::log(&format!("merge result for {session}: {msg}"));
                let owner = self.workspace_owner(&session);
                self.notify(
                    Notice::new(tone, Text::tr(tr::merge_notice_title))
                        .body(msg)
                        .session(owner)
                        .toast(),
                );
                Task::none()
            }
            FamilyMsg::ToggleFold(id) => {
                if !self.ui.folded_families.remove(&id) {
                    self.ui.folded_families.insert(id);
                }
                self.save_ui();
                Task::none()
            }
        }
    }

    pub(crate) fn child_card<'a>(&'a self, f: &'a ChildForm) -> El<'a> {
        let parent = self.live(&f.parent);
        let ptitle = parent.map(|l| l.info.title.as_str()).unwrap_or("?");
        let mut c = Column::<El<'a>>::new()
            .spacing(10)
            .push(text(tr::child_dialog_title(ptitle)).size(18).font(style::bold()).boxed())
            .push(
                column![
                    text(tr::session_name_label()).size(12).font(style::bold()),
                    text_input(tr::child_title_placeholder(), &f.title)
                        .on_input(|v| fm(FamilyMsg::Title(v)))
                        .on_submit(fm(FamilyMsg::Submit))
                        .size(13)
                        .padding([5, 8])
                        .style(style::input),
                ]
                .spacing(4)
                .boxed(),
            );
        let choice = |shared: bool, label: String, hint: String| -> El<'a> {
            let on = f.shared == shared;
            button(
                column![
                    text(label).size(13).font(style::bold()),
                    text(hint).size(11).color(style::DIM),
                ]
                .spacing(2),
            )
            .padding([6, 10])
            .width(Fill)
            // Same height for both, whatever their hints wrap to.
            .height(CHOICE_H)
            .style(style::choice(on))
            .on_press(fm(FamilyMsg::Shared(shared)))
            .boxed()
        };
        c = c.push(
            row![
                choice(false, tr::child_separate(), tr::child_separate_hint()),
                choice(true, tr::child_shared(), tr::child_shared_hint()),
            ]
            .spacing(8)
            .boxed(),
        );
        if !f.shared {
            c = c.push(text(tr::child_repos_label()).size(12).font(style::bold()).boxed());
            for r in parent.map(|l| l.info.repos.as_slice()).unwrap_or(&[]) {
                let id = r.repo.clone();
                c = c.push(
                    checkbox(f.repos.contains(&r.repo))
                        .label(format!("{}  ·  {}", r.name, r.branch))
                        .on_toggle(move |v| fm(FamilyMsg::Repo(id.clone(), v)))
                        .size(15)
                        .text_size(12)
                        .boxed(),
                );
            }
        }
        if let Some(e) = &f.error {
            c = c.push(crate::view::error_box(e));
        }
        let ready = !f.busy && self.connected() && (f.shared || !f.repos.is_empty());
        c = c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                button(text(tr::child_create()).size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press_maybe(ready.then(|| fm(FamilyMsg::Submit))),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .boxed(),
        );
        c.width(520).boxed()
    }
}

/// One line per repo of a merge / sync (user-facing, translated).
fn merge_summary(m: &[RepoMerge]) -> String {
    if m.is_empty() {
        return tr::merge_nothing();
    }
    m.iter()
        .map(|r| match &r.result {
            MergeResult::Merged => tr::merge_merged(&r.name),
            MergeResult::UpToDate => tr::merge_up_to_date(&r.name),
            MergeResult::Conflicts { files } => {
                tr::merge_conflicts(i18n::count(files.len()), &r.name)
            }
            MergeResult::Failed { error } => tr::merge_failed(&r.name, error),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
