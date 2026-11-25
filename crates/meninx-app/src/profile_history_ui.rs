//! The profile's change history on the profiles page (`docs/profile-editing.md`): every change
//! of the profile (this page, a session's `meninx_profile_edit`, a restore) is a version on the
//! server (`configuration.history`); any version can be brought back (`configuration.restore`,
//! itself recorded as a new change). Also the notice when a session changed a profile
//! (`Notify::ConfigurationEdited`).

use iced::widget::{Column, button, column, container, row, space, text};
use iced::{Alignment, Element, Fill, Task, Widget as _};

use meninx_client::{ConfigurationChange, ConfigurationInfo, ProtoError};

use crate::app::{App, Message};
use crate::i18n::{self, tr};
use crate::notices::{Notice, Text};
use crate::style;

type El<'a> = Element<'a, Message>;

/// Changes listed.
const SHOWN: u32 = 30;

#[derive(Debug, Clone)]
pub enum HistoryMsg {
    /// The changes of a profile (slug) arrived.
    Loaded(String, Result<Vec<ConfigurationChange>, ProtoError>),
    /// Arm the inline confirm of "이 버전으로" (`None` = cancel).
    RestoreArm(Option<String>),
    /// Restore the profile (slug) to change `id`.
    Restore(String, String),
    Restored(Result<ConfigurationInfo, ProtoError>),
}

/// The history shown on the profiles page.
#[derive(Debug, Default)]
pub struct HistoryState {
    /// The profile whose history is shown, and its changes (or why they could not be read).
    pub list: Option<(String, Result<Vec<ConfigurationChange>, String>)>,
    /// The change whose restore waits for its confirm.
    pub restore_armed: Option<String>,
    pub busy: bool,
}

impl App {
    /// Read the shown profile's history again (only while the profiles page is open).
    pub(crate) fn load_profile_history(&self) -> Task<Message> {
        if self.ui.page != crate::ui_state::Page::Configurations {
            return Task::none();
        }
        let (Some(c), Some(slug)) = (self.online(), self.profile_shown().map(|p| p.slug.clone()))
        else {
            return Task::none();
        };
        Task::perform(
            async move {
                let r = c.configuration_history(&slug, SHOWN).await;
                (slug, r)
            },
            |(slug, r)| Message::ProfileHistory(HistoryMsg::Loaded(slug, r)),
        )
    }

    pub(crate) fn on_profile_history(&mut self, m: HistoryMsg) -> Task<Message> {
        let h = &mut self.settings.history;
        match m {
            HistoryMsg::Loaded(slug, r) => {
                if h.list.as_ref().is_none_or(|(s, _)| *s != slug) {
                    h.restore_armed = None;
                }
                h.list = Some((slug, r.map_err(|e| i18n::proto_error(&e))));
                Task::none()
            }
            HistoryMsg::RestoreArm(id) => {
                h.restore_armed = id;
                Task::none()
            }
            HistoryMsg::Restore(slug, id) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let h = &mut self.settings.history;
                h.busy = true;
                h.restore_armed = None;
                Task::perform(
                    async move { c.restore_configuration(&slug, &id).await },
                    |r| Message::ProfileHistory(HistoryMsg::Restored(r)),
                )
            }
            HistoryMsg::Restored(r) => {
                self.settings.history.busy = false;
                match r {
                    Ok(_) => {
                        // The editor starts again from the restored profile.
                        self.settings.cfg_edit = None;
                        self.notify(Notice::success(Text::tr(tr::profile_restored)).toast());
                    }
                    Err(e) => {
                        self.notify(
                            Notice::error(Text::tr(tr::profile_restored))
                                .body(i18n::proto_error(&e))
                                .toast(),
                        );
                    }
                }
                Task::batch([self.refresh(), self.load_profile_history()])
            }
        }
    }

    /// A session changed a profile: tell the user (the lists reload on the
    /// `ConfigurationsChanged` that comes with it).
    pub(crate) fn profile_edited_notice(
        &mut self,
        name: String,
        session_title: String,
        summary: String,
    ) -> Task<Message> {
        let body = tr::profile_edited_body(&session_title, &summary);
        self.notify(
            Notice::info(Text::tr(move || tr::profile_edited(&name)))
                .body(body)
                .toast(),
        );
        self.load_profile_history()
    }

    /// The "변경 이력" section's body for profile `slug`.
    pub(crate) fn profile_history_view(&self, slug: &str, online: bool) -> El<'_> {
        let h = &self.settings.history;
        let changes = match &h.list {
            Some((s, Ok(list))) if s == slug => list,
            Some((s, Err(e))) if s == slug => {
                return text(tr::profile_history_failed(e)).size(11).color(style::RED).boxed();
            }
            _ => return text("…").size(11).color(style::FAINT).boxed(),
        };
        if changes.is_empty() {
            return text(tr::profile_history_none()).size(11).color(style::FAINT).boxed();
        }
        let now = crate::history::now_secs();
        let can = online && !h.busy;
        let mut list: Column<El<'_>> = Column::new().spacing(8);
        for (i, c) in changes.iter().enumerate() {
            let who = match c.source.as_str() {
                "agent" => tr::profile_change_agent(
                    c.session_title
                        .as_deref()
                        .or(c.session.as_deref())
                        .unwrap_or("?"),
                ),
                "restore" => tr::profile_change_restore(),
                "import" => tr::profile_change_import(),
                _ => tr::profile_change_ui(),
            };
            let when = crate::history::relative_time(
                i64::try_from(c.time).unwrap_or(i64::MAX),
                now,
            );
            let mut info: Column<El<'_>> = column![
                row![
                    text(who).size(12).font(style::bold()),
                    text(when).size(11).color(style::DIM),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .boxed(),
            ]
            .spacing(2)
            .width(Fill);
            // Only an agent's line is its own text; the others are fixed English texts.
            if c.source == "agent" && !c.summary.is_empty() {
                info = info.push(text(c.summary.as_str()).size(12).color(style::TEXT).boxed());
            }
            if !c.files.is_empty() {
                info = info.push(
                    text(c.files.join(", "))
                        .size(10)
                        .color(style::FAINT)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End)
                        .boxed(),
                );
            }
            // The newest change is the current version: nothing to go back to.
            let action: El<'_> = if i == 0 {
                space().width(0).boxed()
            } else if h.restore_armed.as_deref() == Some(c.id.as_str()) {
                row![
                    text(tr::profile_restore_question()).size(11).color(style::DIM),
                    button(text(tr::profile_restore_confirm()).size(11))
                        .padding([3, 10])
                        .style(style::primary)
                        .on_press_maybe(can.then(|| {
                            Message::ProfileHistory(HistoryMsg::Restore(slug.to_owned(), c.id.clone()))
                        })),
                    button(text(tr::cancel()).size(11))
                        .padding([3, 10])
                        .style(style::secondary)
                        .on_press(Message::ProfileHistory(HistoryMsg::RestoreArm(None))),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .boxed()
            } else {
                button(text(tr::profile_restore()).size(11))
                    .padding([3, 10])
                    .style(style::secondary)
                    .on_press_maybe(
                        can.then(|| Message::ProfileHistory(HistoryMsg::RestoreArm(Some(c.id.clone())))),
                    )
                    .boxed()
            };
            list = list.push(
                container(
                    row![info, action].spacing(10).align_y(Alignment::Center),
                )
                .width(Fill)
                .boxed(),
            );
        }
        list.boxed()
    }
}
