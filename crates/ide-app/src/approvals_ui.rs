//! Approvals in the app (`docs/approvals.md`): the daemon asks (`Notify::AgentRequest`,
//! `ide.approval`) before an agent acts beyond its session; the app shows one question at a
//! time (others wait in a queue) and answers `once` / `session` / `always`, or refuses (also
//! when the dialog is closed). Settings -> 권한 lists the kept grants with 취소.

use std::collections::VecDeque;

use iced::widget::{Column, button, column, container, row, space, text};
use iced::{Alignment, Element, Fill, Task, Widget as _};
use ide_client::{ApprovalInfo, ProtoError};

use crate::app::{App, Message, Modal};
use crate::i18n::tr;
use crate::style;

type El<'a> = Element<'a, Message>;

/// A question of the daemon.
#[derive(Debug, Clone)]
pub struct ApprovalForm {
    /// The request to answer.
    pub request: u64,
    pub capability: String,
    pub requester_title: String,
    pub target_title: String,
    /// What exactly (e.g. the message), shown as is.
    pub detail: String,
}

/// Questions waiting behind the shown one, and the settings list.
#[derive(Default)]
pub struct ApprovalsState {
    pub queue: VecDeque<ApprovalForm>,
    pub list: Vec<ApprovalInfo>,
    pub error: Option<String>,
    /// Seconds a question waits for the user (`None`: until answered); `None` until loaded.
    pub wait: Option<Option<u64>>,
}

#[derive(Debug, Clone)]
pub enum ApprovalMsg {
    /// The dialog's answer: `once` / `session` / `always`.
    Answer(&'static str),
    Listed(Result<Vec<ApprovalInfo>, ProtoError>),
    Revoke(String),
    Revoked(Result<(), ProtoError>),
    /// How long questions wait (loaded with the list).
    Wait(Result<Option<u64>, ProtoError>),
    SetWait(Option<u64>),
    WaitSet(Result<(), ProtoError>),
}

fn am(m: ApprovalMsg) -> Message {
    Message::Approval(m)
}

/// What the user is asked to allow, in words.
fn question(f: &ApprovalForm) -> String {
    let (who, whom) = (f.requester_title.as_str(), f.target_title.as_str());
    match f.capability.as_str() {
        "session.steer" => tr::approval_steer(who, whom),
        "session.interrupt" => tr::approval_interrupt(who, whom),
        // The target is the folder itself (`docs/write-guard.md`).
        "folder.write" => tr::approval_folder_write(who, whom),
        "terminal.input" => tr::approval_terminal_input(who, whom),
        ide_proto::browser::CAP_ACT => tr::approval_browser_act(who, whom),
        ide_proto::browser::CAP_READ => tr::approval_browser_read(who, whom),
        other => tr::approval_other(who, other, whom),
    }
}

/// The waits the user can choose (seconds; `None`: until answered).
type Label = fn() -> String;
const WAIT_CHOICES: [(Option<u64>, Label); 5] = [
    (Some(60), tr::approvals_wait_1m),
    (Some(300), tr::approvals_wait_5m),
    (Some(900), tr::approvals_wait_15m),
    (Some(3600), tr::approvals_wait_1h),
    (None, tr::approvals_wait_none),
];

/// A capability in a short phrase (the settings list).
fn capability_text(c: &str) -> String {
    match c {
        "session.steer" => tr::approval_cap_steer(),
        "session.interrupt" => tr::approval_cap_interrupt(),
        "folder.write" => tr::approval_cap_folder_write(),
        "terminal.input" => tr::approval_cap_terminal_input(),
        ide_proto::browser::CAP_ACT => tr::approval_cap_browser_act(),
        ide_proto::browser::CAP_READ => tr::approval_cap_browser_read(),
        other => other.to_owned(),
    }
}

impl App {
    /// The daemon asks for an approval (answered when the user decides).
    pub(crate) fn approval_request(&mut self, request: u64, args: &serde_json::Value) -> Task<Message> {
        let s = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let title = |t: String, id: String| if t.is_empty() { id } else { t };
        let f = ApprovalForm {
            request,
            capability: s("capability"),
            requester_title: title(s("requester_title"), s("requester")),
            target_title: title(s("target_title"), s("target")),
            detail: s("detail"),
        };
        crate::script::log(&format!("approval asked: {} {} -> {}", f.capability, f.requester_title, f.target_title));
        self.approvals.queue.push_back(f);
        self.next_approval();
        Task::none()
    }

    /// Show the next waiting question when no dialog is open.
    pub(crate) fn next_approval(&mut self) {
        if self.modal.is_none()
            && let Some(f) = self.approvals.queue.pop_front()
        {
            self.modal = Some(Modal::Approval(f));
        }
    }

    /// The daemon stopped waiting for `request` (answered elsewhere, timed out, its caller
    /// went): its question goes, whether shown or waiting, and is not answered any more.
    pub(crate) fn agent_request_ended(&mut self, request: u64, timed_out: bool) -> Task<Message> {
        self.approvals.queue.retain(|f| f.request != request);
        let shown = match &self.modal {
            Some(Modal::Approval(f)) => f.request == request,
            Some(Modal::WidgetRegister(f)) => f.request == request,
            _ => false,
        };
        if shown {
            crate::script::log(&format!("question {request} ended (timed out: {timed_out})"));
            self.modal = None;
            if timed_out {
                self.notify(crate::notices::Notice::warning(crate::notices::Text::tr(tr::approval_timed_out)).toast());
            }
            self.next_approval();
        }
        Task::none()
    }

    fn reply(&self, request: u64, result: Result<String, String>) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let _ = c.agent_reply(request, result).await;
            },
            |_| (),
        )
        .discard()
    }

    /// The question's dialog closes without an answer: refused.
    pub(crate) fn approval_closed(&self) -> Task<Message> {
        match &self.modal {
            Some(Modal::Approval(f)) => self.reply(f.request, Err("refused by the user".into())),
            _ => Task::none(),
        }
    }

    /// Load the kept grants (settings).
    pub(crate) fn load_approvals(&self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let c2 = c.clone();
        Task::batch([
            Task::perform(async move { c.approval_list().await }, |r| am(ApprovalMsg::Listed(r))),
            Task::perform(async move { c2.approval_wait().await }, |r| am(ApprovalMsg::Wait(r))),
        ])
    }

    pub(crate) fn on_approval(&mut self, m: ApprovalMsg) -> Task<Message> {
        match m {
            ApprovalMsg::Answer(answer) => {
                let Some(Modal::Approval(f)) = self.modal.take() else {
                    return Task::none();
                };
                crate::script::log(&format!("approval answered: {answer}"));
                let t = self.reply(f.request, Ok(answer.to_owned()));
                self.next_approval();
                Task::batch([t, self.load_approvals()])
            }
            ApprovalMsg::Listed(r) => {
                match r {
                    Ok(list) => {
                        self.approvals.list = list;
                        self.approvals.error = None;
                    }
                    Err(e) => self.approvals.error = Some(crate::i18n::proto_error(&e)),
                }
                Task::none()
            }
            ApprovalMsg::Revoke(id) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                Task::perform(async move { c.approval_revoke(&id).await }, |r| am(ApprovalMsg::Revoked(r)))
            }
            ApprovalMsg::Revoked(r) => {
                if let Err(e) = r {
                    self.approvals.error = Some(crate::i18n::proto_error(&e));
                }
                self.load_approvals()
            }
            ApprovalMsg::Wait(r) => {
                match r {
                    Ok(w) => self.approvals.wait = Some(w),
                    Err(e) => self.approvals.error = Some(crate::i18n::proto_error(&e)),
                }
                Task::none()
            }
            ApprovalMsg::SetWait(secs) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                // Shown at once; the reload after the change shows what the daemon keeps.
                self.approvals.wait = Some(secs);
                crate::script::log(&format!("approval wait: {secs:?}"));
                Task::perform(async move { c.approval_set_wait(secs).await }, |r| am(ApprovalMsg::WaitSet(r)))
            }
            ApprovalMsg::WaitSet(r) => {
                if let Err(e) = r {
                    self.approvals.error = Some(crate::i18n::proto_error(&e));
                }
                self.load_approvals()
            }
        }
    }

    /// The question.
    pub(crate) fn approval_card<'a>(&'a self, f: &'a ApprovalForm) -> El<'a> {
        let mut c: Column<El<'a>> = column![
            text(tr::approval_title()).size(18).font(style::bold()).boxed(),
            text(question(f)).size(13).boxed(),
        ]
        .spacing(10);
        if !f.detail.is_empty() {
            c = c.push(
                container(text(f.detail.clone()).size(12).font(style::mono()))
                    .padding([6, 10])
                    .width(Fill)
                    .style(style::card)
                    .boxed(),
            );
        }
        c = c.push(text(tr::approval_note()).size(12).color(style::DIM).boxed());
        let b = |label: String, answer: &'static str, primary: bool| {
            button(text(label).size(13))
                .padding([6, 12])
                .style(if primary { style::primary } else { style::secondary })
                .on_press(am(ApprovalMsg::Answer(answer)))
                .boxed()
        };
        c.push(
            row![
                button(text(tr::approval_refuse()).size(13))
                    .padding([6, 12])
                    .style(style::secondary)
                    .on_press(Message::CloseModal),
                space().width(Fill),
                b(tr::approval_once(), "once", false),
                b(tr::approval_session(), "session", false),
                b(tr::approval_always(), "always", true),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .boxed(),
        )
        .width(560)
        .boxed()
    }

    /// Settings -> 권한: the kept grants.
    pub(crate) fn approvals_section(&self) -> El<'_> {
        let mut c: Column<El<'_>> = Column::new().spacing(8);
        c = c.push(text(tr::approvals_label()).size(11).color(style::DIM).boxed());
        c = c.push(text(tr::approvals_note()).size(12).color(style::DIM).boxed());
        if let Some(e) = &self.approvals.error {
            c = c.push(crate::view::error_box(e));
        }
        let online = self.connected();
        // How long a question waits for the answer.
        if let Some(wait) = self.approvals.wait {
            let mut choices = row![].spacing(6);
            for (secs, label) in WAIT_CHOICES {
                let on = wait == secs;
                choices = choices.push(
                    button(text(label()).size(12))
                        .padding([4, 10])
                        .style(if on { style::primary } else { style::secondary })
                        .on_press_maybe((online && !on).then_some(am(ApprovalMsg::SetWait(secs)))),
                );
            }
            c = c.push(
                column![
                    text(tr::approvals_wait_label()).size(11).color(style::DIM),
                    choices,
                    text(tr::approvals_wait_note()).size(12).color(style::DIM),
                ]
                .spacing(6)
                .boxed(),
            );
        }
        if self.approvals.list.is_empty() {
            c = c.push(text(tr::approvals_none()).size(13).color(style::DIM).boxed());
        }
        for g in &self.approvals.list {
            // A session that went (its grants go with it; a race shows its gone name).
            let name = |t: &str| if t.is_empty() { tr::widget_session_gone() } else { t.to_owned() };
            let how = if g.always { tr::approvals_always() } else { tr::approvals_until_restart() };
            c = c.push(
                row![
                    column![
                        text(tr::approvals_row(
                            &name(&g.requester_title),
                            &capability_text(&g.capability),
                            &name(&g.target_title),
                        ))
                        .size(13),
                        text(how).size(11).color(style::DIM),
                    ]
                    .spacing(2)
                    .width(Fill),
                    button(text(tr::approvals_revoke()).size(12))
                        .padding([4, 10])
                        .style(style::secondary)
                        .on_press_maybe(online.then(|| am(ApprovalMsg::Revoke(g.id.clone())))),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .boxed(),
            );
        }
        c.boxed()
    }
}
