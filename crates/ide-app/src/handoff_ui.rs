//! Session handoff in the app (`docs/session-handoff.md`): the 세션 인수인계 dialog (method,
//! agent / account / configuration), its running notice and the result of `Notify::Handoff`,
//! and 인수인계 취소 from the row menu.

use iced::widget::{Column, button, column, row, scrollable, space, text};
use iced::{Alignment, Element, Fill, Task, Widget as _};
use ide_client::{HandoffMethod, HandoffResult, ProtoError, SessionInfo, SessionKind};

use crate::app::{App, Message, Modal};
use crate::family_ui::AgentFieldMsgs;
use crate::i18n::{self, tr};
use crate::notices::{Notice, NoticeKey, TaskState, Text};
use crate::style;

type El<'a> = Element<'a, Message>;

/// The 세션 인수인계 dialog.
pub struct HandoffForm {
    pub session: String,
    pub method: HandoffMethod,
    /// The new conversation's agent: provider, account, configuration (the session's at first).
    pub provider: String,
    pub account: Option<String>,
    pub configuration: String,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum HandoffMsg {
    /// Open the dialog for this session.
    Open(String),
    Method(HandoffMethod),
    Provider(String),
    /// Account slug; empty = the CLI's default login.
    Account(String),
    Configuration(String),
    Submit,
    /// The server took (or refused) the handoff of this session.
    Started(String, Box<Result<SessionInfo, ProtoError>>),
    /// 인수인계 취소 of this session.
    Cancel(String),
    Cancelled(String, Box<Result<SessionInfo, ProtoError>>),
}

fn hm(m: HandoffMsg) -> Message {
    Message::Handoff(m)
}

/// The methods in the dialog's order.
const METHODS: [HandoffMethod; 4] = [
    HandoffMethod::Auto,
    HandoffMethod::Previous,
    HandoffMethod::Summarize,
    HandoffMethod::Transcript,
];

/// The dialog's name of a method.
fn method_label(m: HandoffMethod) -> String {
    match m {
        HandoffMethod::Auto => tr::handoff_auto(),
        HandoffMethod::Previous => tr::handoff_previous(),
        HandoffMethod::Summarize => tr::handoff_summarize(),
        HandoffMethod::Transcript => tr::handoff_transcript(),
    }
}

fn method_hint(m: HandoffMethod) -> String {
    match m {
        HandoffMethod::Auto => tr::handoff_auto_hint(),
        HandoffMethod::Previous => tr::handoff_previous_hint(),
        HandoffMethod::Summarize => tr::handoff_summarize_hint(),
        HandoffMethod::Transcript => tr::handoff_transcript_hint(),
    }
}

/// The one notice that follows a session's handoff (running, then its result).
fn notice_key(session: &str) -> NoticeKey {
    NoticeKey::Other(format!("handoff:{session}"))
}

impl App {
    /// Can `id` be handed off now (an agent session without a running handoff)?
    pub(crate) fn can_hand_off(&self, id: &str) -> bool {
        self.live(id)
            .is_some_and(|l| l.info.kind == SessionKind::Agent && l.info.handoff.is_none())
    }

    pub(crate) fn on_handoff(&mut self, m: HandoffMsg) -> Task<Message> {
        match m {
            HandoffMsg::Open(session) => {
                if !self.connected() || !self.can_hand_off(&session) {
                    return Task::none();
                }
                let Some(info) = self.live(&session).map(|l| &l.info) else {
                    return Task::none();
                };
                let (provider, account, configuration) =
                    (info.provider.clone(), info.account.clone(), info.configuration.clone());
                self.modal = Some(Modal::Handoff(HandoffForm {
                    session,
                    method: HandoffMethod::Auto,
                    provider,
                    account,
                    configuration,
                    error: None,
                    busy: false,
                }));
                Task::none()
            }
            HandoffMsg::Method(v) => {
                if let Some(Modal::Handoff(f)) = &mut self.modal {
                    f.method = v;
                }
                Task::none()
            }
            HandoffMsg::Provider(p) => {
                // The session's account for the session's provider, else the provider's default.
                let own_account = match &self.modal {
                    Some(Modal::Handoff(f)) => self
                        .live(&f.session)
                        .filter(|l| l.info.provider == p)
                        .map(|l| l.info.account.clone()),
                    _ => None,
                };
                let account = own_account.unwrap_or_else(|| self.default_account(&p));
                if let Some(Modal::Handoff(f)) = &mut self.modal {
                    f.provider = p;
                    f.account = account;
                }
                Task::none()
            }
            HandoffMsg::Account(a) => {
                if let Some(Modal::Handoff(f)) = &mut self.modal {
                    f.account = (!a.is_empty()).then_some(a);
                }
                Task::none()
            }
            HandoffMsg::Configuration(v) => {
                if let Some(Modal::Handoff(f)) = &mut self.modal {
                    f.configuration = v;
                }
                Task::none()
            }
            HandoffMsg::Submit => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::Handoff(f)) = &mut self.modal else {
                    return Task::none();
                };
                if f.busy {
                    return Task::none();
                }
                f.busy = true;
                f.error = None;
                let (session, method) = (f.session.clone(), f.method);
                let agent = ide_client::ChildAgent {
                    provider: f.provider.clone(),
                    account: f.account.clone(),
                    configuration: f.configuration.clone(),
                };
                // The session's own agent goes as `None` (unchanged).
                let same = self.live(&session).is_some_and(|l| {
                    l.info.provider == agent.provider
                        && l.info.account == agent.account
                        && l.info.configuration == agent.configuration
                });
                let agent = (!same).then_some(agent);
                crate::script::log(&format!(
                    "handoff of {session}: method={method:?} agent={agent:?}"
                ));
                let id = session.clone();
                Task::perform(
                    async move { c.session_handoff(&id, method, agent).await },
                    move |r| hm(HandoffMsg::Started(session.clone(), Box::new(r))),
                )
            }
            HandoffMsg::Started(session, r) => match *r {
                Ok(info) => {
                    crate::script::log(&format!(
                        "handoff of {session} started: {:?}",
                        info.handoff
                    ));
                    if matches!(&self.modal, Some(Modal::Handoff(f)) if f.session == session) {
                        self.modal = None;
                    }
                    if let Some(l) = self.live_mut(&session) {
                        l.info = info;
                    }
                    let name = self.session_name(&session).unwrap_or_else(|| session.clone());
                    self.notify(
                        Notice::info(Text::tr(tr::notice_handoff_running))
                            .body(name)
                            .session(session.clone())
                            .key(notice_key(&session))
                            .task(TaskState::Running)
                            .quiet(),
                    );
                    Task::none()
                }
                Err(e) => {
                    crate::script::log(&format!("handoff of {session} refused: {e:?}"));
                    if let Some(Modal::Handoff(f)) = &mut self.modal {
                        f.busy = false;
                        f.error = Some(i18n::proto_error(&e));
                    }
                    Task::none()
                }
            },
            HandoffMsg::Cancel(session) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                crate::script::log(&format!("cancel handoff of {session}"));
                let id = session.clone();
                Task::perform(
                    async move { c.session_handoff_cancel(&id).await },
                    move |r| hm(HandoffMsg::Cancelled(session.clone(), Box::new(r))),
                )
            }
            HandoffMsg::Cancelled(session, r) => {
                let key = notice_key(&session);
                match *r {
                    Ok(info) => {
                        crate::script::log(&format!("handoff of {session} cancelled"));
                        if let Some(l) = self.live_mut(&session) {
                            l.info = info;
                        }
                        let name = self.session_name(&session).unwrap_or_else(|| session.clone());
                        self.notify(
                            Notice::info(Text::tr(tr::notice_handoff_cancelled))
                                .body(name)
                                .session(session)
                                .key(key)
                                .task(TaskState::Done)
                                .toast()
                                .quiet(),
                        );
                    }
                    Err(e) => {
                        crate::script::log(&format!("cancel handoff of {session} failed: {e:?}"));
                        self.notify(
                            Notice::error(Text::tr(tr::notice_handoff_cancel_failed))
                                .body(i18n::proto_error(&e))
                                .session(session)
                                .toast(),
                        );
                    }
                }
                Task::none()
            }
        }
    }

    /// `Notify::Handoff`: a session's handoff ended (started here or by another client). Its
    /// running notice, if any, becomes the result.
    pub(crate) fn handoff_ended(&mut self, session: String, result: HandoffResult) -> Task<Message> {
        crate::script::log(&format!("handoff of {session} ended: {result:?}"));
        if let Some(l) = self.live_mut(&session) {
            l.info.handoff = None;
        }
        let name = self.session_name(&session).unwrap_or_else(|| session.clone());
        let n = match result {
            HandoffResult::Done { method } => {
                Notice::success(Text::tr(move || tr::notice_handoff_done(&method_label(method))))
                    .body(name)
                    .task(TaskState::Done)
                    .toast()
                    .quiet()
            }
            HandoffResult::Failed { error } => Notice::error(Text::tr(tr::notice_handoff_failed))
                .body(format!("{name}: {}", i18n::proto_error(&error)))
                .task(TaskState::Failed)
                .toast(),
        };
        self.notify(n.session(session.clone()).key(notice_key(&session)));
        // The session's provider, account, configuration and conversation changed.
        self.refresh()
    }

    pub(crate) fn handoff_card<'a>(&'a self, f: &'a HandoffForm) -> El<'a> {
        let title = self
            .live(&f.session)
            .map(|l| l.info.title.as_str())
            .unwrap_or("?");
        let mut methods = Column::<El<'a>>::new().spacing(6);
        for m in METHODS {
            methods = methods.push(
                button(
                    column![
                        text(method_label(m)).size(13).font(style::bold()),
                        text(method_hint(m)).size(11).color(style::DIM),
                    ]
                    .spacing(2),
                )
                .padding([6, 10])
                .width(Fill)
                .style(style::choice(f.method == m))
                .on_press(hm(HandoffMsg::Method(m)))
                .boxed(),
            );
        }
        let fields = column![
            text(tr::handoff_explain()).size(12).color(style::DIM),
            column![crate::view::label(tr::handoff_method_label()), methods].spacing(8),
            self.agent_fields(
                &f.provider,
                f.account.as_deref(),
                &f.configuration,
                AgentFieldMsgs {
                    provider: |v| hm(HandoffMsg::Provider(v)),
                    account: |v| hm(HandoffMsg::Account(v)),
                    configuration: |v| hm(HandoffMsg::Configuration(v)),
                },
            )
            .boxed(),
        ]
        .spacing(12);
        // The title and the buttons stay put; the fields between them scroll when the dialog
        // would be taller than the window.
        let mut c = column![
            text(tr::handoff_dialog_title(title)).size(18).font(style::bold()),
            scrollable(fields).height(crate::view::fit_up_to_room()).spacing(6),
        ]
        .spacing(10);
        if let Some(e) = &f.error {
            c = c.push(crate::view::error_box(e));
        }
        let ready = !f.busy && self.connected();
        c = c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                button(text(tr::handoff_submit()).size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press_maybe(ready.then(|| hm(HandoffMsg::Submit))),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .boxed(),
        );
        c.width(520).boxed()
    }
}
