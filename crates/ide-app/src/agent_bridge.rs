//! The app's part of the agent bridge (`docs/agent-bridge.md`): a session's agent called a
//! bridge tool that acts in the UI; the server hands it over as `Notify::AgentRequest` and the
//! first app to answer (`agent.reply`) wins. Tools the app does not know are refused, so an
//! older app next to a newer server fails the call instead of leaving the agent waiting.

use iced::Task;
use ide_client::{AGENT_TOOL_SHOW_MESSAGE, AGENT_TOOL_WIDGET, APPROVAL_REQUEST};

use crate::app::{App, Message};

impl App {
    pub(crate) fn on_agent_request(
        &mut self,
        request: u64,
        session: String,
        tool: String,
        args: String,
    ) -> Task<Message> {
        let args: serde_json::Value = serde_json::from_str(&args).unwrap_or_default();
        if tool == APPROVAL_REQUEST {
            // Answered when the user decides (`approvals_ui.rs`).
            return self.approval_request(request, &args);
        }
        if tool == AGENT_TOOL_WIDGET {
            // Answered when done (after fetching the widget's info).
            return self.agent_widget(request, session, &args);
        }
        let result: Result<String, String> = match tool.as_str() {
            AGENT_TOOL_SHOW_MESSAGE => {
                let message = args
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                crate::script::log(&format!("agent {session}: show message {message:?}"));
                match self.live(&session) {
                    Some(_) if !message.is_empty() => {
                        // Agent text, shown as is (user content, not translated); unread in the
                        // notification centre (it is meant for the user).
                        self.notify(
                            crate::notices::Notice::info(message)
                                .session(session.clone())
                                .open(Message::Select(session.clone()))
                                .toast(),
                        );
                        Ok("shown in the ide".to_owned())
                    }
                    Some(_) => Err("`message` is empty".to_owned()),
                    None => Err("this session is not open in the ide window".to_owned()),
                }
            }
            other => Err(format!("this ide window cannot carry out `{other}`")),
        };
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
}
