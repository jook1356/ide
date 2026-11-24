//! Settings -> 에이전트 자동 응답 (`docs/auto-answers.md`): per agent, the dialogs ide answers
//! by itself, each on or off. The rules (dialog texts, keys) are the providers' (`ide-agents`
//! screen manifests); the app only names the answer kinds, provider-neutral.

use iced::widget::{Column, checkbox, column, text};
use iced::{Element, Task, Widget as _};
use ide_client::ProtoError;

use crate::app::{App, Message};
use crate::i18n::tr;
use crate::style;

type El<'a> = Element<'a, Message>;

#[derive(Debug, Clone)]
pub enum AutoAnswerMsg {
    /// Turn (provider, answer) on / off.
    Set(String, String, bool),
    Saved(Result<(), ProtoError>),
}

/// An answer kind's name and what it does, the same for every agent (unknown ids: the id).
fn names(id: &str) -> (String, String) {
    match id {
        "trust_workspace" => (tr::auto_answer_trust_workspace(), tr::auto_answer_trust_workspace_note()),
        "trust_added_dir" => (tr::auto_answer_trust_added_dir(), tr::auto_answer_trust_added_dir_note()),
        other => (other.to_owned(), String::new()),
    }
}

impl App {
    pub(crate) fn on_auto_answer(&mut self, m: AutoAnswerMsg) -> Task<Message> {
        match m {
            AutoAnswerMsg::Set(provider, answer, enabled) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                // Shown at once; the next provider listing confirms it.
                if let Some(a) = self
                    .providers
                    .iter_mut()
                    .find(|p| p.id == provider)
                    .and_then(|p| p.answers.iter_mut().find(|a| a.id == answer))
                {
                    a.enabled = enabled;
                }
                Task::perform(
                    async move { c.set_auto_answer(&provider, &answer, enabled).await },
                    |r| Message::AutoAnswer(AutoAnswerMsg::Saved(r)),
                )
            }
            AutoAnswerMsg::Saved(r) => {
                self.auto_answer_error = r.err().map(|e| crate::i18n::proto_error(&e));
                self.refresh()
            }
        }
    }

    /// The settings section (nothing when no agent has answers).
    pub(crate) fn auto_answers_section(&self) -> Option<El<'_>> {
        let with: Vec<_> = self.providers.iter().filter(|p| !p.answers.is_empty()).collect();
        if with.is_empty() {
            return None;
        }
        let online = self.connected();
        let mut c: Column<El<'_>> = column![
            text(tr::auto_answers_label()).size(11).color(style::DIM).boxed(),
            text(tr::auto_answers_note()).size(12).color(style::DIM).boxed(),
        ]
        .spacing(8);
        if let Some(e) = &self.auto_answer_error {
            c = c.push(crate::view::error_box(e));
        }
        for p in with {
            c = c.push(text(p.name.clone()).size(13).font(style::bold()).boxed());
            for a in &p.answers {
                let (name, note) = names(&a.id);
                let (pid, aid) = (p.id.clone(), a.id.clone());
                let mut b = checkbox(a.enabled).label(name).size(15).text_size(13);
                if online {
                    b = b.on_toggle(move |v| Message::AutoAnswer(AutoAnswerMsg::Set(pid.clone(), aid.clone(), v)));
                }
                c = c.push(b.boxed());
                if !note.is_empty() {
                    c = c.push(
                        iced::widget::container(text(note).size(11).color(style::DIM))
                            .padding(iced::Padding {
                                left: 24.0,
                                ..iced::Padding::ZERO
                            })
                            .boxed(),
                    );
                }
            }
        }
        Some(c.boxed())
    }
}
