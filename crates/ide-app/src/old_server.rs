//! An incompatible local server (another protocol, e.g. after 빌드 후 재시작 kept the old
//! daemon): the app cannot talk to it, so it cannot ask it to stop either. The connection
//! status offers 서버 재시작: the daemon named in `<data>/ide-server.pid` (only that one) is ended
//! by pid with its process tree, then reconnecting starts the app's own server.

use iced::widget::{button, column, row, space, text};
use iced::{Element, Fill, Task, Widget as _};

use crate::app::{App, Message, Modal, StopServerForm};
use crate::i18n::tr;
use crate::style;
use crate::view::error_box;

#[derive(Debug, Clone)]
pub enum OldServerMsg {
    Ask,
    Confirm,
    Stopped(Result<(), String>),
}

/// End the daemon of `data_dir` by its pid file (and its children: the sessions' processes).
fn stop_daemon(data_dir: &std::path::Path) -> Result<(), String> {
    let pid: u32 = std::fs::read_to_string(data_dir.join("ide-server.pid"))
        .map_err(|e| e.to_string())?
        .trim()
        .parse()
        .map_err(|_| "bad pid file".to_owned())?;
    kill_tree(pid)
}

#[cfg(windows)]
fn kill_tree(pid: u32) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    // 128: no such process (already gone) is fine.
    match out.status.code() {
        Some(0 | 128) => Ok(()),
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
    }
}

#[cfg(not(windows))]
fn kill_tree(pid: u32) -> Result<(), String> {
    // The daemon runs in its own process group (`spawn_daemon`): end the group.
    let out = std::process::Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

impl App {
    pub(crate) fn on_old_server(&mut self, m: OldServerMsg) -> Task<Message> {
        match m {
            OldServerMsg::Ask => {
                if self.server_outdated {
                    self.modal = Some(Modal::RestartOldServer(StopServerForm::default()));
                }
                Task::none()
            }
            OldServerMsg::Confirm => {
                let Some(Modal::RestartOldServer(f)) = &mut self.modal else {
                    return Task::none();
                };
                f.busy = true;
                f.error = None;
                let data = self.data_dir.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || stop_daemon(&data))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    |r| Message::OldServer(OldServerMsg::Stopped(r)),
                )
            }
            OldServerMsg::Stopped(Ok(())) => {
                crate::script::log("incompatible server stopped by the user");
                self.modal = None;
                self.server_outdated = false;
                // Reconnecting starts this app's server.
                match &self.link {
                    Some(l) => l.client.reconnect(),
                    None => self.connect(),
                }
                Task::none()
            }
            OldServerMsg::Stopped(Err(e)) => {
                if let Some(Modal::RestartOldServer(f)) = &mut self.modal {
                    f.busy = false;
                    f.error = Some(tr::old_server_failed(&e));
                }
                Task::none()
            }
        }
    }

    pub(crate) fn old_server_card<'a>(&'a self, f: &'a StopServerForm) -> Element<'a, Message> {
        let mut c = column![
            text(tr::old_server_title()).size(18).font(style::bold()),
            text(tr::old_server_body()).size(13).color(style::DIM),
        ]
        .spacing(10);
        if let Some(e) = &f.error {
            c = c.push(error_box(e));
        }
        c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                button(
                    text(if f.busy {
                        tr::stopping()
                    } else {
                        tr::old_server_confirm()
                    })
                    .size(13),
                )
                .padding([6, 14])
                .style(style::danger)
                .on_press_maybe(
                    (!f.busy).then_some(Message::OldServer(OldServerMsg::Confirm)),
                ),
            ]
            .spacing(8)
            .boxed(),
        )
        .width(460)
        .boxed()
    }
}
