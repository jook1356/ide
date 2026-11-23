//! A profile's own skills in the profile editor (the server keeps them in
//! `<data>/configurations/<slug>/skills/<name>/`; `ide-core/src/skill.rs`). Like Hermes Desk's
//! agent 스킬 tab (`SkillsPanel.tsx`): a list with on/off switches, 내용 보기, delete with an
//! inline confirm; instead of Desk's hub install, a new skill is written here (name +
//! description → `SKILL.md`) or copied from a folder on the server, and `SKILL.md` is editable.
//! Every change is applied at once (not with the editor's 저장); new sessions pick it up.

use iced::widget::{Column, Row, button, column, container, row, space, text, text_editor, text_input};
use iced::{Alignment, Element, Fill, Task, Widget as _};

use ide_client::{ConfigurationInfo, ProtoError};

use crate::app::{App, Message, Modal, PickPurpose, PickRootForm};
use crate::i18n::{self, tr};
use crate::style;
use crate::view::{icon_button, tip};

type El<'a> = Element<'a, Message>;

#[derive(Debug, Clone)]
pub enum SkillMsg {
    /// "새 스킬" form: open / close, fields, create.
    NewOpen(bool),
    NewName(String),
    NewDescription(String),
    NewCreate,
    /// "폴더에서 가져오기": the server folder picker.
    Import,
    /// A folder picked for import.
    Picked(String),
    Toggle(String, bool),
    /// Open `SKILL.md` in the editor dialog.
    Open(String),
    Opened(String, String, Result<String, ProtoError>),
    EditAction(text_editor::Action),
    EditSave,
    /// Delete: arm the inline confirm (`None` = cancel), then confirm.
    DeleteArm(Option<String>),
    Delete(String),
    /// A change applied (the profile as it is now).
    Done(Result<ConfigurationInfo, ProtoError>),
}

/// The editing state of the skills of the profile being edited.
#[derive(Debug, Default)]
pub struct SkillsEdit {
    /// The "새 스킬" form: (name, description).
    pub new: Option<(String, String)>,
    /// The skill whose delete waits for its confirm.
    pub delete_armed: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
}

/// The `SKILL.md` editor dialog.
pub struct SkillEditForm {
    pub configuration: String,
    pub name: String,
    pub content: text_editor::Content,
    pub busy: bool,
    pub error: Option<String>,
}

impl App {
    /// The slug of the profile being edited.
    fn edited_slug(&self) -> Option<String> {
        self.settings.cfg_edit.as_ref().map(|e| e.base.slug.clone())
    }

    fn skills_edit(&mut self) -> Option<&mut SkillsEdit> {
        self.settings.cfg_edit.as_mut().map(|e| &mut e.skills)
    }

    /// Run a skill change; the answer replaces the profile in the list.
    fn skill_call<F, Fut>(&mut self, f: F) -> Task<Message>
    where
        F: FnOnce(ide_client::Client, String) -> Fut,
        Fut: std::future::Future<Output = Result<ConfigurationInfo, ProtoError>> + Send + 'static,
    {
        let (Some(c), Some(slug)) = (self.online(), self.edited_slug()) else {
            return Task::none();
        };
        if let Some(s) = self.skills_edit() {
            s.busy = true;
            s.error = None;
        }
        Task::perform(f(c, slug), |r| Message::Skill(SkillMsg::Done(r)))
    }

    pub(crate) fn on_skill(&mut self, m: SkillMsg) -> Task<Message> {
        match m {
            SkillMsg::NewOpen(open) => {
                if let Some(s) = self.skills_edit() {
                    s.new = open.then(Default::default);
                    s.error = None;
                }
                Task::none()
            }
            SkillMsg::NewName(v) => {
                if let Some((n, _)) = self.skills_edit().and_then(|s| s.new.as_mut()) {
                    *n = v;
                }
                Task::none()
            }
            SkillMsg::NewDescription(v) => {
                if let Some((_, d)) = self.skills_edit().and_then(|s| s.new.as_mut()) {
                    *d = v;
                }
                Task::none()
            }
            SkillMsg::NewCreate => {
                let Some((name, desc)) = self.skills_edit().and_then(|s| s.new.clone()) else {
                    return Task::none();
                };
                if name.trim().is_empty() {
                    return Task::none();
                }
                self.skill_call(move |c, slug| async move {
                    c.skill_create(&slug, name.trim(), desc.trim()).await
                })
            }
            SkillMsg::Import => {
                let Some(slug) = self.edited_slug() else {
                    return Task::none();
                };
                if self.online().is_none() {
                    return Task::none();
                }
                self.modal = Some(Modal::PickRoot(PickRootForm {
                    purpose: PickPurpose::Skill(slug),
                    ..PickRootForm::default()
                }));
                self.pick_go(None)
            }
            SkillMsg::Picked(path) => {
                crate::script::log(&format!("skill: import {path:?}"));
                self.skill_call(move |c, slug| async move { c.skill_import(&slug, &path).await })
            }
            SkillMsg::Toggle(name, on) => {
                self.skill_call(move |c, slug| async move { c.skill_enable(&slug, &name, on).await })
            }
            SkillMsg::Open(name) => {
                let (Some(c), Some(slug)) = (self.online(), self.edited_slug()) else {
                    return Task::none();
                };
                Task::perform(
                    {
                        let (slug, name) = (slug.clone(), name.clone());
                        async move { c.skill_read(&slug, &name).await }
                    },
                    move |r| Message::Skill(SkillMsg::Opened(slug.clone(), name.clone(), r)),
                )
            }
            SkillMsg::Opened(slug, name, r) => {
                match r {
                    Ok(t) => {
                        self.modal = Some(Modal::SkillEdit(SkillEditForm {
                            configuration: slug,
                            name,
                            content: text_editor::Content::with_text(&t),
                            busy: false,
                            error: None,
                        }));
                    }
                    Err(e) => {
                        if let Some(s) = self.skills_edit() {
                            s.error = Some(i18n::proto_error(&e));
                        }
                    }
                }
                Task::none()
            }
            SkillMsg::EditAction(a) => {
                if let Some(Modal::SkillEdit(f)) = &mut self.modal {
                    f.content.perform(a);
                }
                Task::none()
            }
            SkillMsg::EditSave => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::SkillEdit(f)) = &mut self.modal else {
                    return Task::none();
                };
                f.busy = true;
                f.error = None;
                let (slug, name, text) = (f.configuration.clone(), f.name.clone(), f.content.text());
                Task::perform(
                    async move { c.skill_write(&slug, &name, &text).await },
                    |r| Message::Skill(SkillMsg::Done(r)),
                )
            }
            SkillMsg::DeleteArm(name) => {
                if let Some(s) = self.skills_edit() {
                    s.delete_armed = name;
                }
                Task::none()
            }
            SkillMsg::Delete(name) => {
                self.skill_call(move |c, slug| async move { c.skill_delete(&slug, &name).await })
            }
            SkillMsg::Done(r) => {
                crate::script::log(&format!(
                    "skill change: {:?}",
                    r.as_ref().map(|c| c.skills.iter().map(|s| (&s.name, s.enabled)).collect::<Vec<_>>())
                ));
                let err = r.as_ref().err().map(i18n::proto_error);
                if let Some(Modal::SkillEdit(f)) = &mut self.modal {
                    f.busy = false;
                    match &err {
                        Some(e) => f.error = Some(e.clone()),
                        None => self.modal = None,
                    }
                }
                if let Some(s) = self.skills_edit() {
                    s.busy = false;
                    s.delete_armed = None;
                    if err.is_none() {
                        s.new = None;
                    }
                    s.error = err;
                }
                if let Ok(info) = r {
                    if let Some(c) = self.configs.iter_mut().find(|c| c.slug == info.slug) {
                        c.skills = info.skills.clone();
                    }
                    if let Some(e) = self.settings.cfg_edit.as_mut()
                        && e.base.slug == info.slug
                    {
                        e.base.skills = info.skills;
                    }
                }
                Task::none()
            }
        }
    }

    /// The skills part of the profile editor.
    pub(crate) fn skills_editor<'a>(
        &'a self,
        e: &'a crate::app::ConfigEdit,
        online: bool,
    ) -> El<'a> {
        let s = &e.skills;
        let can = online && !s.busy;
        let skills = &e.base.skills;
        let on = skills.iter().filter(|k| k.enabled).count();
        let mut col = column![
            row![
                text(tr::skills_count(i18n::count(skills.len()), i18n::count(on)))
                    .size(10)
                    .color(style::FAINT),
                space().width(Fill),
                button(text(tr::skill_new()).size(11))
                    .padding([2, 8])
                    .style(style::secondary)
                    .on_press_maybe(can.then_some(Message::Skill(SkillMsg::NewOpen(true)))),
                button(text(tr::skill_import()).size(11))
                    .padding([2, 8])
                    .style(style::secondary)
                    .on_press_maybe(can.then_some(Message::Skill(SkillMsg::Import))),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ]
        .spacing(4);
        if let Some((name, desc)) = &s.new {
            let create = can && !name.trim().is_empty();
            col = col.push(
                container(
                    column![
                        text_input(tr::skill_name_placeholder(), name)
                            .on_input(|v| Message::Skill(SkillMsg::NewName(v)))
                            .on_submit_maybe(
                                create.then_some(Message::Skill(SkillMsg::NewCreate))
                            )
                            .size(12)
                            .padding([4, 8]),
                        text_input(tr::skill_description_placeholder(), desc)
                            .on_input(|v| Message::Skill(SkillMsg::NewDescription(v)))
                            .on_submit_maybe(
                                create.then_some(Message::Skill(SkillMsg::NewCreate))
                            )
                            .size(12)
                            .padding([4, 8]),
                        row![
                            text(tr::skill_new_note()).size(10).color(style::FAINT).width(Fill),
                            button(text(tr::cancel()).size(11))
                                .padding([2, 8])
                                .style(style::secondary)
                                .on_press(Message::Skill(SkillMsg::NewOpen(false))),
                            button(text(tr::skill_create()).size(11))
                                .padding([2, 8])
                                .style(style::primary)
                                .on_press_maybe(
                                    create.then_some(Message::Skill(SkillMsg::NewCreate))
                                ),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    ]
                    .spacing(4),
                )
                .padding(8)
                .style(style::card)
                .boxed(),
            );
        }
        if skills.is_empty() {
            col = col.push(text(tr::skills_none()).size(11).color(style::FAINT).boxed());
        }
        for k in skills {
            let name = k.name.clone();
            let toggle = iced::widget::toggler(k.enabled)
                .on_toggle_maybe(can.then(|| {
                    let name = name.clone();
                    move |v| Message::Skill(SkillMsg::Toggle(name.clone(), v))
                }))
                .size(14);
            let armed = s.delete_armed.as_deref() == Some(k.name.as_str());
            let actions: El<'_> = if armed {
                row![
                    button(text(tr::delete()).size(11))
                        .padding([2, 8])
                        .style(style::danger)
                        .on_press_maybe(can.then_some(Message::Skill(SkillMsg::Delete(name.clone())))),
                    button(text(tr::cancel()).size(11))
                        .padding([2, 8])
                        .style(style::secondary)
                        .on_press(Message::Skill(SkillMsg::DeleteArm(None))),
                ]
                .spacing(4)
                .boxed()
            } else {
                row![
                    tip(
                        button(text(tr::skill_edit()).size(11))
                            .padding([2, 8])
                            .style(style::secondary)
                            .on_press_maybe(online.then_some(Message::Skill(SkillMsg::Open(name.clone()))))
                            .boxed(),
                        tr::skill_edit_tip(),
                    ),
                    icon_button(
                        "✕",
                        can.then_some(Message::Skill(SkillMsg::DeleteArm(Some(name.clone())))),
                        tr::skill_delete_tip(),
                    ),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .boxed()
            };
            let mut title = Row::<El<'_>>::new().spacing(6).push(text(k.title.as_str()).size(12).boxed());
            if k.title != k.name {
                title = title.push(text(k.name.as_str()).size(10).font(style::mono()).color(style::FAINT).boxed());
            }
            let mut info = Column::<El<'_>>::new().push(title.boxed()).spacing(1).width(Fill);
            if !k.description.is_empty() {
                info = info.push(
                    text(k.description.as_str())
                        .size(11)
                        .color(style::DIM)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End)
                        .boxed(),
                );
            }
            col = col.push(
                row![toggle, info, actions]
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .boxed(),
            );
        }
        if let Some(err) = &s.error {
            col = col.push(text(err.as_str()).size(11).color(style::RED).boxed());
        }
        col.boxed()
    }

    /// The `SKILL.md` editor dialog.
    pub(crate) fn skill_modal_card<'a>(&'a self, f: &'a SkillEditForm) -> El<'a> {
        let mut c = column![
            text(tr::skill_edit_title()).size(18).font(style::bold()),
            text(format!("{}/SKILL.md", f.name))
                .size(12)
                .font(style::mono())
                .color(style::DIM),
            text_editor(&f.content)
                .on_action(|a| Message::Skill(SkillMsg::EditAction(a)))
                .font(style::mono())
                .size(12)
                .height(360),
        ]
        .spacing(10);
        if let Some(e) = &f.error {
            c = c.push(text(e.as_str()).size(12).color(style::RED).boxed());
        }
        c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                button(text(tr::save()).size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press_maybe(
                        (!f.busy && self.connected()).then_some(Message::Skill(SkillMsg::EditSave))
                    ),
            ]
            .spacing(8)
            .boxed(),
        )
        .width(640)
        .boxed()
    }
}
